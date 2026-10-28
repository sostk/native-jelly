//! Jellyfin trickplay addressing and bounded manifest selection. No image or network work on UI.
//! Wire contract: docs/jf-api/jellyfin-openapi-12.0.json, TrickplayInfoDto and GetTrickplayTileImage.
use serde::Deserialize;
use serde_json::Value;

/// A playback/version identity; never fall back to another version's thumbnails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub item: String,
    pub source: String,
    /// Digest of the logical playback id: ephemeral keys never carry a credential or raw session.
    pub generation: String,
}

fn guid(s: &str) -> Option<String> {
    let n = super::ids::normalize(s);
    (n.len() == 32
        && matches!(s.len(), 32 | 36)
        && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
    .then_some(n)
}

impl Owner {
    pub(crate) fn new(item: &str, source: &str, session: &str) -> Option<Self> {
        if session.is_empty() {
            return None;
        }
        let digest = nj_base::sha256::sha256(session.as_bytes());
        Some(Self {
            item: guid(item)?,
            source: guid(source)?,
            generation: digest[..8].iter().map(|b| format!("{b:02x}")).collect(),
        })
    }

    pub(crate) fn key(&self, ms: i64) -> String {
        format!(
            "/nj/trickplay/{}/{}/{}/{}",
            self.item,
            self.source,
            self.generation,
            ms.max(0)
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub owner: Owner,
    pub ms: i64,
}
impl Request {
    pub(crate) fn parse(key: &str) -> Option<Self> {
        let mut p = key.strip_prefix("/nj/trickplay/")?.split('/');
        let item = guid(p.next()?)?;
        let source = guid(p.next()?)?;
        let generation = p.next()?;
        if generation.len() != 16 || !generation.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let ms = p.next()?.parse::<i64>().ok().filter(|v| *v >= 0)?;
        if p.next().is_some() {
            return None;
        }
        Some(Self {
            owner: Owner {
                item,
                source,
                generation: generation.into(),
            },
            ms,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct Spec {
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub thumbnail_count: u32,
    pub interval: u32,
}

/// Largest supported sheet: the default 320×180, 10×10 sheet is 5.76 MP.
/// A single sheet is decoded on a worker; only one thumbnail enters the GPU cache.
const SHEET_PIXELS_MAX: u64 = 6_291_456;
impl Spec {
    pub(crate) fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.tile_width > 0
            && self.tile_height > 0
            && self.interval > 0
            && self.thumbnail_count > 0
            && u64::from(self.width) * u64::from(self.tile_width) <= 4096
            && u64::from(self.height) * u64::from(self.tile_height) <= 4096
            && u64::from(self.width)
                * u64::from(self.tile_width)
                * u64::from(self.height)
                * u64::from(self.tile_height)
                <= SHEET_PIXELS_MAX
    }
    pub(crate) fn tile(self, ms: i64) -> Option<Tile> {
        if !self.valid() {
            return None;
        }
        let n =
            (ms.max(0) as u64 / u64::from(self.interval)).min(u64::from(self.thumbnail_count - 1));
        let per_sheet = u64::from(self.tile_width) * u64::from(self.tile_height);
        let within = n % per_sheet;
        Some(Tile {
            sheet: n / per_sheet,
            x: (within % u64::from(self.tile_width)) as u32 * self.width,
            y: (within / u64::from(self.tile_width)) as u32 * self.height,
            w: self.width,
            h: self.height,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tile {
    pub sheet: u64,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Choose the closest economical resolution, rejecting malformed entries independently.
/// The manifest is keyed first by media source id and then thumbnail width.
pub(crate) fn select(item: &Value, owner: &Owner) -> Option<Spec> {
    let sources = item.get("Trickplay")?.as_object()?;
    let widths = sources
        .iter()
        .find(|(k, _)| guid(k).as_deref() == Some(&owner.source))?
        .1
        .as_object()?;
    widths
        .iter()
        .filter_map(|(key, value)| {
            let spec: Spec = serde_json::from_value(value.clone()).ok()?;
            (key.parse::<u32>().ok() == Some(spec.width) && spec.valid()).then_some(spec)
        })
        .min_by_key(|s| (s.width > 320, s.width.abs_diff(320), s.height))
}

pub(crate) fn sheet_path(owner: &Owner, spec: Spec, sheet: u64) -> String {
    format!(
        "/Videos/{}/Trickplay/{}/{sheet}.jpg?mediaSourceId={}",
        owner.item, spec.width, owner.source
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const ITEM: &str = "0123456789abcdef0123456789abcdef";
    const SOURCE: &str = "11111111111111111111111111111111";
    fn owner() -> Owner {
        Owner::new(ITEM, SOURCE, "playback").unwrap()
    }
    fn spec() -> Spec {
        Spec {
            width: 320,
            height: 180,
            tile_width: 10,
            tile_height: 10,
            thumbnail_count: 205,
            interval: 10_000,
        }
    }
    fn value(s: Spec) -> Value {
        json!({"Width":s.width,"Height":s.height,"TileWidth":s.tile_width,"TileHeight":s.tile_height,"ThumbnailCount":s.thumbnail_count,"Interval":s.interval})
    }
    #[test]
    fn trickplay_tile_boundaries_and_last_partial_sheet() {
        let s = spec();
        assert_eq!(
            s.tile(-1).unwrap(),
            Tile {
                sheet: 0,
                x: 0,
                y: 0,
                w: 320,
                h: 180
            }
        );
        assert_eq!(s.tile(9_999), s.tile(0));
        assert_eq!(s.tile(10_000).unwrap().x, 320);
        assert_eq!(s.tile(100_000).unwrap().y, 180);
        assert_eq!(
            s.tile(999_999).unwrap(),
            Tile {
                sheet: 0,
                x: 2880,
                y: 1620,
                w: 320,
                h: 180
            }
        );
        assert_eq!(
            s.tile(1_000_000).unwrap(),
            Tile {
                sheet: 1,
                x: 0,
                y: 0,
                w: 320,
                h: 180
            }
        );
        assert_eq!(
            s.tile(i64::MAX).unwrap(),
            Tile {
                sheet: 2,
                x: 1280,
                y: 0,
                w: 320,
                h: 180
            }
        );
    }
    #[test]
    fn trickplay_rejects_zero_overflow_and_oversized_sheets() {
        for s in [
            Spec {
                interval: 0,
                ..spec()
            },
            Spec {
                thumbnail_count: 0,
                ..spec()
            },
            Spec {
                tile_width: 0,
                ..spec()
            },
            Spec {
                width: u32::MAX,
                ..spec()
            },
            Spec {
                tile_height: u32::MAX,
                ..spec()
            },
            Spec {
                width: 640,
                height: 360,
                ..spec()
            },
        ] {
            assert!(!s.valid());
            assert!(s.tile(0).is_none());
        }
    }
    #[test]
    fn trickplay_chooses_only_the_played_version_and_valid_resolution() {
        let v = json!({"Trickplay":{SOURCE:{"320":value(spec()),"640":value(Spec {width:640,tile_width:1,tile_height:1,..spec()}),"bad":{},"160":null},ITEM:{"320":value(spec())}}});
        assert_eq!(select(&v, &owner()), Some(spec()));
        let other = Owner::new(ITEM, "22222222222222222222222222222222", "playback").unwrap();
        assert_eq!(
            select(&v, &other),
            None,
            "never show a different cut's timeline"
        );
        assert_eq!(select(&json!({"Trickplay":null}), &owner()), None);
    }
    #[test]
    fn trickplay_keys_round_trip_and_bind_playback_without_credentials() {
        let o = owner();
        let key = o.key(123_000);
        assert_eq!(
            Request::parse(&key),
            Some(Request {
                owner: o.clone(),
                ms: 123_000
            })
        );
        assert!(!key.contains("playback"));
        assert_ne!(
            o.key(0),
            Owner::new(ITEM, SOURCE, "next playback").unwrap().key(0)
        );
        for key in [
            format!("{key}/extra"),
            o.key(0).replace("/0", "/-1"),
            "/nj/trickplay/x".into(),
        ] {
            assert!(Request::parse(&key).is_none());
        }
        assert!(sheet_path(&o, spec(), 2)
            .contains(&format!("/Trickplay/320/2.jpg?mediaSourceId={SOURCE}")));
    }
}

/// A small render-fact cache. The worker publishes the manifest; the UI can then quantize keys
/// to its true interval without fetching/uploading the same still on every scrub tick.
/// This is not playback state: absence or eviction affects optional images only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayoutId {
    pub srv: u16,
    pub token: u32,
    pub cache: u64,
    pub owner: Owner,
}
#[derive(Default)]
pub(crate) struct Layouts(std::collections::VecDeque<(LayoutId, Option<Spec>)>);
impl Layouts {
    pub(crate) fn get(&self, id: &LayoutId) -> Option<Option<Spec>> {
        self.0.iter().find(|(k, _)| k == id).map(|(_, s)| *s)
    }
    fn put(&mut self, id: LayoutId, spec: Option<Spec>) {
        self.0.retain(|(k, _)| k != &id);
        while self.0.len() >= 4 {
            self.0.pop_front();
        }
        self.0.push_back((id, spec));
    }
}
fn layouts() -> &'static std::sync::Mutex<Layouts> {
    static L: std::sync::OnceLock<std::sync::Mutex<Layouts>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}
pub(crate) fn known(id: &LayoutId) -> Option<Option<Spec>> {
    layouts().lock().ok()?.get(id)
}
pub(crate) fn publish(id: LayoutId, spec: Option<Spec>) {
    if let Ok(mut l) = layouts().lock() {
        l.put(id, spec);
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    fn trickplay_layout_cache_is_bounded_and_account_playback_scoped() {
        let id = LayoutId {
            srv: 0,
            token: 1,
            cache: 1,
            owner: Owner::new(
                "0123456789abcdef0123456789abcdef",
                "0123456789abcdef0123456789abcdef",
                "first",
            )
            .unwrap(),
        };
        let mut l = Layouts::default();
        l.put(id.clone(), None);
        assert_eq!(l.get(&id), Some(None));
        assert_eq!(
            l.get(&LayoutId {
                token: 2,
                ..id.clone()
            }),
            None
        );
        assert_eq!(
            l.get(&LayoutId {
                srv: 1,
                ..id.clone()
            }),
            None
        );
        assert_eq!(
            l.get(&LayoutId {
                cache: 2,
                ..id.clone()
            }),
            None
        );
        for i in 2..=5 {
            l.put(
                LayoutId {
                    token: i,
                    ..id.clone()
                },
                None,
            );
        }
        assert_eq!(l.0.len(), 4);
        assert_eq!(l.get(&id), None);
    }
}
