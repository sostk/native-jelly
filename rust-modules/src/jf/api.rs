//! The Jellyfin operations behind `plex::Client`: each method answers what its PMS namesake does,
//! in the same Plex-shaped types, so every store above the facade reads one vocabulary.
//!
//! `Client::jf()` hands one of these out when the client's origin is a Jellyfin seat
//! ([`super::seat`]); the op methods in `plex/` delegate at their first line. Requests go through
//! `Client::jf_send` — the same [`crate::http`] door, origin, resolve pin and credential gate as
//! every PMS request — with the `Authorization: MediaBrowser …` header instead of a query token.
use super::models::*;
use super::{convert, ids, seat, url};
use crate::http::Method;
use crate::plex::collections::CollectionOutcome;
use crate::plex::{
    Client, Hub, LibrarySection, MediaContainer, Meta, MetaType, Metadata, SectionQuery,
    SortOption, Tag,
};
use serde::de::DeserializeOwned;

/// `Fields=` for list endpoints: what a poster grid and a shelf draw, without the per-item
/// `MediaSources` a detail page needs (those make a 50-row page ~40x larger).
const LIST_FIELDS: &str =
    "PrimaryImageAspectRatio,Overview,Genres,DateCreated,ChildCount,RecursiveItemCount,ProviderIds,Taglines";
/// …and for rows that may be PLAYED straight from the list (episodes, Up Next, Continue
/// Watching): those need their media sources so the route can pick a part without a detail read.
const PLAYABLE_FIELDS: &str =
    "PrimaryImageAspectRatio,Overview,Genres,DateCreated,ChildCount,RecursiveItemCount,ProviderIds,Taglines,MediaSources,Chapters";

/// Percent-encoded query assembly — the JF twin of `plex::QueryBuilder`, over the same encoder.
pub(crate) struct Q {
    path: String,
    parts: Vec<String>,
}

impl Q {
    pub(crate) fn new(path: impl Into<String>) -> Self {
        Self { path: path.into(), parts: Vec::new() }
    }
    pub(crate) fn s(mut self, k: &str, v: &str) -> Self {
        if !v.is_empty() {
            self.parts.push(format!("{k}={}", crate::plex::urlenc_str(v)));
        }
        self
    }
    pub(crate) fn i(mut self, k: &str, v: i64) -> Self {
        self.parts.push(format!("{k}={v}"));
        self
    }
    pub(crate) fn b(self, k: &str, v: bool) -> Self {
        self.s(k, if v { "true" } else { "false" })
    }
    pub(crate) fn build(self) -> String {
        if self.parts.is_empty() {
            self.path
        } else {
            let sep = if self.path.contains('?') { '&' } else { '?' };
            format!("{}{sep}{}", self.path, self.parts.join("&"))
        }
    }
}

/// The Plex `sort` expression (`titleSort:asc`, `addedAt:desc`, `a:desc,b`) → `(SortBy, SortOrder)`.
pub(crate) fn sort_by(plex_sort: &str) -> (String, &'static str) {
    let mut fields = Vec::new();
    let mut order = "Ascending";
    for (i, part) in plex_sort.split(',').filter(|p| !p.is_empty()).enumerate() {
        let (key, dir) = part.split_once(':').unwrap_or((part, "asc"));
        if i == 0 && dir == "desc" {
            order = "Descending";
        }
        let mapped: &[&str] = match key {
            "titleSort" | "title" => &["SortName"],
            "addedAt" => &["DateCreated"],
            "originallyAvailableAt" | "year" => &["PremiereDate", "ProductionYear"],
            "audienceRating" => &["CommunityRating"],
            "rating" => &["CriticRating"],
            "lastViewedAt" => &["DatePlayed"],
            "viewCount" => &["PlayCount"],
            "duration" => &["Runtime"],
            "random" => &["Random"],
            "showOrder" => &["SeriesSortName", "ParentIndexNumber", "IndexNumber"],
            "seasonOrder" => &["SeriesSortName", "IndexNumber"],
            _ => &[],
        };
        for m in mapped {
            if !fields.contains(m) {
                fields.push(*m);
            }
        }
    }
    if fields.is_empty() {
        fields.push("SortName");
    }
    if !fields.contains(&"SortName") && !fields.contains(&"Random") {
        fields.push("SortName");
    }
    (fields.join(","), order)
}

fn sort_option(key: &str, title: &str, default_direction: &str) -> SortOption {
    SortOption { key: key.into(), desc_key: String::new(), default_direction: default_direction.into(), title: title.into() }
}

/// The sort menu a section advertises for `jf_type` — the server-driven `Meta` PMS sends with
/// `includeMeta=1`, synthesized from what `/Items` can actually order by.
fn sort_menu(jf_type: &str) -> Meta {
    let mut sort = match jf_type {
        "Episode" => vec![SortOption {
            key: "showOrder".into(),
            desc_key: "showOrder:desc".into(),
            default_direction: "asc".into(),
            title: "Show".into(),
        }],
        "Season" => vec![sort_option("seasonOrder", "Show", "asc")],
        _ => Vec::new(),
    };
    sort.extend([
        sort_option("titleSort", "Title", "asc"),
        sort_option("addedAt", "Date Added", "desc"),
        sort_option("originallyAvailableAt", "Release Date", "desc"),
        sort_option("audienceRating", "Rating", "desc"),
        sort_option("lastViewedAt", "Last Played", "desc"),
        sort_option("duration", "Duration", "desc"),
    ]);
    Meta { types: vec![MetaType { active: 1, sort }] }
}

pub struct Jf<'a> {
    c: &'a Client,
    /// The DeviceId basis for a sign-in, before the origin has a seat to hold it.
    device_user: Option<String>,
}

impl<'a> Jf<'a> {
    pub(crate) fn new(c: &'a Client) -> Self {
        Self { c, device_user: None }
    }

    pub(crate) fn for_sign_in(c: &'a Client, device_user: &str) -> Self {
        Self { c, device_user: Some(device_user.to_string()) }
    }

    fn seat(&self) -> seat::Seat {
        seat::get(self.c.origin()).unwrap_or_default()
    }

    pub(crate) fn origin(&self) -> &crate::plex::Origin {
        self.c.origin()
    }

    pub(crate) fn token(&self) -> String {
        self.c.current_token()
    }

    /// The four device facts plus this user's DeviceId — see [`url::device_id`].
    pub(crate) fn identity(&self) -> url::DeviceIdentity {
        let s = self.seat();
        url::DeviceIdentity {
            client: crate::plex::identity::PRODUCT.to_string(),
            device: crate::plex::identity::device_name().to_string(),
            device_id: url::device_id(self.c.client_id(), self.device_user.as_deref().unwrap_or(&s.device_user)),
            version: crate::plex::identity::VERSION.to_string(),
        }
    }

    pub(crate) fn auth_header(&self) -> String {
        url::authorization(&self.identity(), &self.c.current_token())
    }

    fn send(&self, path: &str, method: Method, body: Option<&[u8]>) -> Option<crate::http::Reply> {
        let auth = self.auth_header();
        let mut headers = vec![crate::http::ACCEPT_JSON, auth.as_str()];
        if body.is_some() {
            headers.push("Content-Type: application/json");
        }
        self.c.jf_send(path, method, &headers, body)
    }

    pub(crate) fn status(&self, path: &str, method: Method, body: Option<&[u8]>) -> Option<i32> {
        self.send(path, method, body).map(|r| r.status)
    }

    fn ok(&self, path: &str, method: Method, body: Option<&[u8]>) -> bool {
        self.send(path, method, body).is_some_and(|r| r.ok())
    }

    pub(crate) fn get<T: DeserializeOwned>(&self, path: &str) -> Option<T> {
        let r = self.send(path, Method::Get, None)?;
        if !r.ok() {
            return None;
        }
        parse(path, &r.body)
    }

    fn get_status<T: DeserializeOwned>(&self, path: &str) -> (Option<i32>, Option<T>) {
        match self.send(path, Method::Get, None) {
            None => (None, None),
            Some(r) if r.ok() => (Some(r.status), parse(path, &r.body)),
            Some(r) => (Some(r.status), None),
        }
    }

    pub(crate) fn post<T: DeserializeOwned>(&self, path: &str, body: &impl serde::Serialize) -> Option<T> {
        let bytes = serde_json::to_vec(body).ok()?;
        let r = self.send(path, Method::Post, Some(&bytes))?;
        if !r.ok() {
            return None;
        }
        parse(path, &r.body)
    }

    pub(crate) fn post_ok(&self, path: &str, body: Option<&impl serde::Serialize>) -> bool {
        match body {
            Some(b) => match serde_json::to_vec(b) {
                Ok(bytes) => self.ok(path, Method::Post, Some(&bytes)),
                Err(_) => false,
            },
            None => self.ok(path, Method::Post, None),
        }
    }

    /// The signed-in user's id, learned once per seat from `GET /Users/Me`.
    pub(crate) fn user_id(&self) -> Option<String> {
        let s = self.seat();
        if !s.user_id.is_empty() {
            return Some(s.user_id);
        }
        let me: UserDto = self.get("/Users/Me")?;
        if me.id.is_empty() {
            return None;
        }
        let id = me.id.clone();
        seat::update(self.c.origin(), |s| {
            s.user_id = me.id;
            if s.user_name.is_empty() {
                s.user_name = me.name;
            }
            if s.server_id.is_empty() {
                s.server_id = me.server_id;
            }
        });
        Some(id)
    }

    fn items(&self, q: Q) -> Option<QueryResult<BaseItemDto>> {
        let q = match self.user_id() {
            Some(uid) => q.s("userId", &uid),
            None => q,
        };
        self.get(&q.build())
    }

    fn items_container(&self, q: Q, section_id: i64) -> Option<MediaContainer> {
        let r = self.items(q)?;
        let rows = r.items.iter().map(|i| convert::item(i, section_id)).collect();
        Some(convert::container(rows, r.total_record_count, r.start_index))
    }

    fn guid(rk: &str) -> Option<String> {
        ids::guid_of_key(rk)
    }

    // ---- server ---------------------------------------------------------------------------

    pub(crate) fn public_info(&self) -> Option<PublicSystemInfo> {
        let info: PublicSystemInfo = self.get("/System/Info/Public")?;
        seat::update(self.c.origin(), |s| {
            s.server_id = info.id.clone();
            s.server_name = info.server_name.clone();
            s.server_version = info.version.clone();
        });
        Some(info)
    }

    pub fn friendly_name(&self) -> Option<String> {
        self.public_info().map(|i| i.server_name).filter(|s| !s.is_empty())
    }

    pub fn machine_identity(&self) -> Option<String> {
        self.public_info().map(|i| ids::normalize(&i.id)).filter(|s| !s.is_empty())
    }

    /// The "server root" read `serverinfo` makes: version only (Jellyfin has no subscription).
    pub fn server_root(&self) -> Option<MediaContainer> {
        let i = self.public_info()?;
        Some(MediaContainer { friendly_name: i.server_name, version: i.version,
            machine_identifier: ids::normalize(&i.id), ..Default::default() })
    }

    // ---- library --------------------------------------------------------------------------

    pub fn sections(&self) -> Option<MediaContainer> {
        let uid = self.user_id()?;
        let views: QueryResult<BaseItemDto> = self.get(&Q::new("/UserViews").s("userId", &uid).build())?;
        let directory: Vec<LibrarySection> = views.items.iter().filter_map(convert::section).collect();
        Some(MediaContainer { size: directory.len() as i64, directory, ..Default::default() })
    }

    /// Which `IncludeItemTypes` a section listing names: the explicit `type=` filter, else the
    /// section's primary type (Movie for a movie library, Series for TV).
    fn section_type(&self, section_guid: &str, filters: &[(String, String)]) -> &'static str {
        if let Some(t) = filters.iter().find(|(k, _)| k == "type").and_then(|(_, v)| v.parse().ok())
            .and_then(convert::jf_type_of_number)
        {
            return t;
        }
        match self.view_kind(section_guid).as_deref() {
            Some("tvshows") => "Series",
            _ => "Movie",
        }
    }

    fn view_kind(&self, section_guid: &str) -> Option<String> {
        let uid = self.user_id()?;
        let views: QueryResult<BaseItemDto> = self.get(&Q::new("/UserViews").s("userId", &uid).build())?;
        views.items.into_iter().find(|v| ids::normalize(&v.id) == section_guid).and_then(|v| v.collection_type)
    }

    pub fn section_items_query(&self, q: &SectionQuery) -> Option<MediaContainer> {
        let section_guid = ids::guid_of(q.section_key)?;
        let jf_type = self.section_type(&section_guid, q.filters);
        let (sort, order) = sort_by(if q.sort.is_empty() {
            match jf_type { "Episode" => "showOrder", "Season" => "seasonOrder", _ => "titleSort" }
        } else {
            q.sort
        });
        let mut b = Q::new("/Items")
            .s("IncludeItemTypes", jf_type)
            .b("Recursive", true)
            .s("SortBy", &sort)
            .s("SortOrder", order)
            .s("Fields", if jf_type == "Episode" { PLAYABLE_FIELDS } else { LIST_FIELDS })
            .s("ImageTypeLimit", "1")
            .s("EnableImageTypes", "Primary,Backdrop,Logo,Thumb")
            .b("EnableTotalRecordCount", true)
            .i("StartIndex", q.start)
            .i("Limit", q.size);
        if jf_type != "BoxSet" {
            b = b.s("ParentId", &section_guid);
        }
        for (k, v) in q.filters {
            b = match k.as_str() {
                "unwatched" | "unwatchedLeaves" if v == "1" => b.s("Filters", "IsUnplayed"),
                "genre" => match v.parse::<i64>().ok().and_then(ids::guid_of) {
                    Some(g) => b.s("GenreIds", &g),
                    None => b.s("Genres", v),
                },
                "firstCharacter" => b.s("NameStartsWith", v),
                _ => b,
            };
        }
        let mut mc = self.items_container(b, q.section_key)?;
        if q.include_meta {
            mc.meta = Some(sort_menu(jf_type));
        }
        Some(mc)
    }

    pub fn section_directory(&self, section_key: i64, directory: &str, metadata_type: Option<i64>) -> Option<MediaContainer> {
        let section_guid = ids::guid_of(section_key)?;
        let jf_type = metadata_type.and_then(convert::jf_type_of_number)
            .unwrap_or_else(|| self.section_type(&section_guid, &[]));
        let rows: Vec<LibrarySection> = match directory {
            "genre" => {
                let f: QueryFilters = self.get(&Q::new("/Items/Filters2")
                    .s("userId", &self.user_id()?)
                    .s("ParentId", &section_guid)
                    .s("IncludeItemTypes", jf_type)
                    .build())?;
                f.genres.iter().map(|g| LibrarySection {
                    key: ids::intern(&g.id).to_string(),
                    kind: "genre".into(),
                    title: g.name.clone(),
                    size: 0,
                }).collect()
            }
            "firstCharacter" => {
                // One sorted read of every name, grouped by its first character — the server's own
                // SortName order, which is what the grid scrolls through.
                let r = self.items(Q::new("/Items")
                    .s("ParentId", &section_guid)
                    .s("IncludeItemTypes", jf_type)
                    .b("Recursive", true)
                    .s("SortBy", "SortName")
                    .s("Fields", "SortName")
                    .b("EnableImages", false)
                    .b("EnableUserData", false))?;
                let mut out: Vec<LibrarySection> = Vec::new();
                for it in &r.items {
                    let name = it.sort_name.as_deref().unwrap_or(&it.name);
                    let c = name.chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or('#');
                    let key = if c.is_ascii_alphabetic() { c.to_string() } else { "#".to_string() };
                    match out.last_mut() {
                        Some(last) if last.key == key => last.size += 1,
                        _ => out.push(LibrarySection { key: key.clone(), kind: String::new(), title: key, size: 1 }),
                    }
                }
                out
            }
            _ => Vec::new(),
        };
        Some(MediaContainer { size: rows.len() as i64, directory: rows, ..Default::default() })
    }

    /// The full item, with its markers (MediaSegments) and — for a show — the next episode.
    pub fn metadata(&self, rk: &str) -> Option<Metadata> {
        let guid = Self::guid(rk)?;
        let uid = self.user_id()?;
        let it: BaseItemDto = self.get(&Q::new(format!("/Items/{guid}")).s("userId", &uid).build())?;
        let mut m = convert::item(&it, 0);
        if matches!(it.kind.as_str(), "Movie" | "Episode" | "Video") {
            if let Some(segs) = self.get::<QueryResult<MediaSegmentDto>>(&format!("/MediaSegments/{guid}")) {
                m.marker = convert::markers(&segs.items, m.duration);
            }
        }
        if it.kind == "Series" {
            if let Some(next) = self.items(Q::new("/Shows/NextUp").s("SeriesId", &guid).i("Limit", 1)
                .s("Fields", PLAYABLE_FIELDS).b("EnableResumable", true))
                .and_then(|r| r.items.into_iter().next())
            {
                m.on_deck = Some(crate::plex::OnDeckHub { metadata: Some(Box::new(convert::item(&next, 0))) });
            }
        }
        Some(m)
    }

    pub fn extras(&self, rk: &str) -> Option<MediaContainer> {
        let guid = Self::guid(rk)?;
        let uid = self.user_id()?;
        let rows: Vec<BaseItemDto> = self.get(&Q::new(format!("/Items/{guid}/SpecialFeatures")).s("userId", &uid).build())?;
        let mut items: Vec<Metadata> = rows.iter().map(|i| convert::item(i, 0)).collect();
        // A local trailer is listed separately from special features on Jellyfin.
        if let Some(trailers) = self.get::<Vec<BaseItemDto>>(&Q::new(format!("/Items/{guid}/LocalTrailers")).s("userId", &uid).build()) {
            for t in &trailers {
                let mut m = convert::item(t, 0);
                m.kind = "clip".into();
                m.subtype = "trailer".into();
                m.extra_type = 1;
                items.insert(0, m);
            }
        }
        let n = items.len() as i64;
        Some(convert::container(items, n, 0))
    }

    pub fn metadata_many(&self, rks: &[&str]) -> Option<MediaContainer> {
        let guids: Vec<String> = rks.iter().filter_map(|rk| Self::guid(rk)).collect();
        if guids.is_empty() {
            return Some(MediaContainer::default());
        }
        let r = self.items(Q::new("/Items").s("Ids", &guids.join(",")).s("Fields", "People"))?;
        // request order, as PMS answers
        let mut rows: Vec<Metadata> = Vec::with_capacity(r.items.len());
        for g in &guids {
            if let Some(it) = r.items.iter().find(|i| ids::normalize(&i.id) == *g) {
                rows.push(convert::item(it, 0));
            }
        }
        let n = rows.len() as i64;
        Some(convert::container(rows, n, 0))
    }

    pub fn children(&self, rk: &str) -> Option<MediaContainer> {
        let guid = Self::guid(rk)?;
        let uid = self.user_id()?;
        let it: BaseItemDto = self.get(&Q::new(format!("/Items/{guid}")).s("userId", &uid).s("Fields", "ChildCount").build())?;
        let q = match it.kind.as_str() {
            "Series" => Q::new(format!("/Shows/{guid}/Seasons")).s("Fields", LIST_FIELDS),
            "Season" => Q::new(format!("/Shows/{}/Episodes", ids::normalize(it.series_id.as_deref().unwrap_or(""))))
                .s("SeasonId", &guid).s("Fields", PLAYABLE_FIELDS),
            _ => Q::new("/Items").s("ParentId", &guid).s("Fields", LIST_FIELDS).s("SortBy", "SortName"),
        };
        self.items_container(q.s("ImageTypeLimit", "1").s("EnableImageTypes", "Primary,Backdrop,Logo,Thumb"), 0)
    }

    pub fn all_leaves(&self, rk: &str) -> Option<MediaContainer> {
        let guid = Self::guid(rk)?;
        self.items_container(Q::new(format!("/Shows/{guid}/Episodes")).s("Fields", PLAYABLE_FIELDS), 0)
    }

    pub fn related(&self, rk: &str) -> Option<MediaContainer> {
        let guid = Self::guid(rk)?;
        let r = self.items(Q::new(format!("/Items/{guid}/Similar")).i("Limit", 20).s("Fields", LIST_FIELDS))?;
        let rows: Vec<Metadata> = r.items.iter().map(|i| convert::item(i, 0)).collect();
        let kind = rows.first().map(|m| m.kind.clone()).unwrap_or_else(|| "movie".into());
        Some(MediaContainer {
            hub: vec![convert::hub(&format!("{kind}.similar"), "More Like This", &kind, "", rows)],
            ..Default::default()
        })
    }

    pub fn person_media(&self, person_id: &str) -> Option<MediaContainer> {
        let guid = if person_id.bytes().all(|b| b.is_ascii_digit()) {
            Self::guid(person_id)?
        } else {
            ids::normalize(person_id)
        };
        self.items_container(Q::new("/Items").s("PersonIds", &guid).b("Recursive", true)
            .s("IncludeItemTypes", "Movie,Series").s("SortBy", "PremiereDate,SortName")
            .s("SortOrder", "Descending").s("Fields", LIST_FIELDS), 0)
    }

    /// `find_by_guid`: does this server hold the item a portable guid names (`imdb://tt…`)?
    pub fn find_by_guid(&self, guid: &str) -> Option<MediaContainer> {
        let (scheme, id) = guid.split_once("://")?;
        if scheme == "jellyfin" {
            let q = Q::new("/Items").s("Ids", id).s("Fields", PLAYABLE_FIELDS);
            return self.items_container(q, 0);
        }
        let flag = match scheme { "imdb" => "HasImdbId", "tmdb" => "HasTmdbId", "tvdb" => "HasTvdbId", _ => return None };
        // `AnyProviderIdEquals` is the provider-id filter; servers that ignore it answer the
        // provider-id-bearing set, so the result is filtered here too.
        let r = self.items(Q::new("/Items").b("Recursive", true).b(flag, true)
            .s("AnyProviderIdEquals", &format!("{}.{id}", provider_key(scheme)))
            .s("IncludeItemTypes", "Movie,Series,Episode").s("Fields", PLAYABLE_FIELDS).i("Limit", 10))?;
        let key = provider_key(scheme);
        let rows: Vec<Metadata> = r.items.iter()
            .filter(|i| i.provider_ids.get(key).is_some_and(|v| v == id))
            .map(|i| convert::item(i, 0)).collect();
        let n = rows.len() as i64;
        Some(convert::container(rows, n, 0))
    }

    // ---- user data ------------------------------------------------------------------------

    pub fn scrobble(&self, rk: &str) -> bool {
        let (Some(guid), Some(uid)) = (Self::guid(rk), self.user_id()) else { return false };
        self.ok(&Q::new(format!("/UserPlayedItems/{guid}")).s("userId", &uid).build(), Method::Post, None)
    }

    pub fn unscrobble(&self, rk: &str) -> bool {
        let (Some(guid), Some(uid)) = (Self::guid(rk), self.user_id()) else { return false };
        self.ok(&Q::new(format!("/UserPlayedItems/{guid}")).s("userId", &uid).build(), Method::Delete, None)
    }

    /// Jellyfin has no hide flag: an item leaves Resume when its position is cleared, so this
    /// resets the resume point (docs/jellyfin-port.md records the difference from PMS).
    pub fn remove_from_continue_watching(&self, rk: &str) -> bool {
        let (Some(guid), Some(uid)) = (Self::guid(rk), self.user_id()) else { return false };
        self.post_ok(
            &Q::new(format!("/UserItems/{guid}/UserData")).s("userId", &uid).build(),
            Some(&serde_json::json!({ "PlaybackPositionTicks": 0 })),
        )
    }

    // ---- hubs -----------------------------------------------------------------------------

    /// Home shelves: one "Recently Added" per library, in the user's view order.
    pub fn home_hubs(&self, count: i64) -> Option<MediaContainer> {
        let uid = self.user_id()?;
        let views: QueryResult<BaseItemDto> = self.get(&Q::new("/UserViews").s("userId", &uid).build())?;
        let mut hubs = Vec::new();
        for v in &views.items {
            let Some(sec) = convert::section(v) else { continue };
            let sid: i64 = sec.key.parse().unwrap_or(0);
            let (identifier, kind) = match sec.kind.as_str() {
                "show" => ("home.television.recent", "mixed"),
                _ => ("home.movies.recent", "movie"),
            };
            let latest: Option<Vec<BaseItemDto>> = self.get(&Q::new("/Items/Latest").s("userId", &uid)
                .s("ParentId", &ids::normalize(&v.id)).i("Limit", count).b("GroupItems", true)
                .s("Fields", LIST_FIELDS).s("ImageTypeLimit", "1")
                .s("EnableImageTypes", "Primary,Backdrop,Logo,Thumb").build());
            let rows: Vec<Metadata> = latest.unwrap_or_default().iter().map(|i| {
                let mut m = convert::item(i, sid);
                m.library_section_title = sec.title.clone();
                m
            }).collect();
            if !rows.is_empty() {
                hubs.push(convert::hub(identifier, &format!("Recently Added in {}", sec.title), kind,
                    &format!("/hubs/home/recentlyAdded?type={}&sectionID={sid}", if kind == "movie" { 1 } else { 2 }), rows));
            }
        }
        Some(MediaContainer { size: hubs.len() as i64, hub: hubs, ..Default::default() })
    }

    pub fn library_hubs(&self, section_key: i64, count: i64) -> Option<MediaContainer> {
        let guid = ids::guid_of(section_key)?;
        let uid = self.user_id()?;
        let kind = self.view_kind(&guid);
        let tv = kind.as_deref() == Some("tvshows");
        let mut hubs = Vec::new();
        let resume = self.items(Q::new("/UserItems/Resume").s("ParentId", &guid).i("Limit", count)
            .s("Fields", PLAYABLE_FIELDS).s("MediaTypes", "Video"));
        if let Some(r) = resume {
            let rows: Vec<Metadata> = r.items.iter().map(|i| convert::item(i, section_key)).collect();
            hubs.push(convert::hub(&format!("{}.inprogress.{section_key}", if tv { "tv" } else { "movie" }),
                "Continue Watching", if tv { "episode" } else { "movie" },
                &format!("/hubs/sections/{section_key}/continueWatching/items"), rows));
        }
        if tv {
            if let Some(r) = self.items(Q::new("/Shows/NextUp").s("ParentId", &guid).i("Limit", count).s("Fields", PLAYABLE_FIELDS)) {
                let rows: Vec<Metadata> = r.items.iter().map(|i| convert::item(i, section_key)).collect();
                hubs.push(convert::hub(&format!("tv.ondeck.{section_key}"), "Next Up", "episode", "", rows));
            }
        }
        let latest: Option<Vec<BaseItemDto>> = self.get(&Q::new("/Items/Latest").s("userId", &uid)
            .s("ParentId", &guid).i("Limit", count).b("GroupItems", true).s("Fields", LIST_FIELDS).build());
        let rows: Vec<Metadata> = latest.unwrap_or_default().iter().map(|i| convert::item(i, section_key)).collect();
        hubs.push(convert::hub(&format!("{}.recentlyadded.{section_key}", if tv { "tv" } else { "movie" }),
            "Recently Added", if tv { "show" } else { "movie" }, "", rows));
        hubs.retain(|h| h.size > 0);
        Some(MediaContainer { size: hubs.len() as i64, hub: hubs, ..Default::default() })
    }

    /// Continue Watching: in-progress items (newest first) followed by Next Up episodes of series
    /// that have none in progress — the merged deck the PMS hub serves.
    pub fn continue_watching(&self, count: i64) -> Option<MediaContainer> {
        let resume = self.items(Q::new("/UserItems/Resume").i("Limit", count).s("Fields", PLAYABLE_FIELDS)
            .s("MediaTypes", "Video").s("ImageTypeLimit", "1").s("EnableImageTypes", "Primary,Backdrop,Logo,Thumb"))?;
        let next = self.items(Q::new("/Shows/NextUp").i("Limit", count).s("Fields", PLAYABLE_FIELDS)
            .b("EnableResumable", false).b("EnableRewatching", false)).unwrap_or_default();
        let mut rows: Vec<Metadata> = resume.items.iter().map(|i| convert::item(i, 0)).collect();
        let in_progress_series: std::collections::HashSet<String> = resume.items.iter()
            .filter_map(|i| i.series_id.as_deref().map(ids::normalize)).collect();
        for it in &next.items {
            if rows.len() as i64 >= count {
                break;
            }
            if it.series_id.as_deref().map(ids::normalize).is_some_and(|s| in_progress_series.contains(&s)) {
                continue;
            }
            rows.push(convert::item(it, 0));
        }
        Some(MediaContainer {
            hub: vec![convert::hub("home.continue", "Continue Watching", "mixed", "/hubs/continueWatching/items", rows)],
            ..Default::default()
        })
    }

    pub fn promoted(&self) -> Option<MediaContainer> {
        Some(MediaContainer::default())
    }

    pub fn search(&self, query: &str, limit: i64, _section_id: i64) -> Option<MediaContainer> {
        let limit = if limit > 0 { limit } else { 10 };
        let r = self.items(Q::new("/Items").s("searchTerm", query).b("Recursive", true)
            .s("IncludeItemTypes", "Movie,Series,Episode,BoxSet").i("Limit", limit * 4)
            .s("Fields", LIST_FIELDS))?;
        let mut hubs = Vec::new();
        for (t, kind, title) in [("Movie", "movie", "Movies"), ("Series", "show", "Shows"),
            ("Episode", "episode", "Episodes"), ("BoxSet", "collection", "Collections")]
        {
            let rows: Vec<Metadata> = r.items.iter().filter(|i| i.kind == t).take(limit as usize)
                .map(|i| convert::item(i, 0)).collect();
            hubs.push(convert::hub(kind, title, kind, "", rows));
        }
        let people: Option<QueryResult<BaseItemDto>> = self.get(&Q::new("/Persons").s("searchTerm", query)
            .i("Limit", limit).s("userId", &self.user_id().unwrap_or_default()).build());
        let tags: Vec<Tag> = people.map(|p| p.items).unwrap_or_default().iter().map(|p| Tag {
            tag: p.name.clone(),
            id: ids::intern(&p.id),
            tag_key: ids::normalize(&p.id),
            thumb: p.image_tags.get("Primary").map(|t| format!("/library/metadata/{}/thumb/{t}", ids::intern(&p.id)))
                .unwrap_or_default(),
            ..Default::default()
        }).collect();
        hubs.push(Hub {
            kind: "actor".into(),
            hub_identifier: "actor".into(),
            title: "Cast & Crew".into(),
            size: tags.len() as i64,
            total_size: tags.len() as i64,
            directory: tags,
            ..Default::default()
        });
        Some(MediaContainer { size: hubs.len() as i64, hub: hubs, ..Default::default() })
    }

    // ---- collections ----------------------------------------------------------------------

    /// BoxSets are server-wide on Jellyfin, not per library; the section argument is kept so the
    /// rows can be stamped with the library that asked.
    pub(crate) fn section_collections(&self, section: i64, start: i64, size: i64) -> CollectionOutcome {
        let q = Q::new("/Items").s("IncludeItemTypes", "BoxSet").b("Recursive", true).s("SortBy", "SortName")
            .s("Fields", LIST_FIELDS).b("EnableTotalRecordCount", true).i("StartIndex", start).i("Limit", size);
        self.collection_page(q, section)
    }

    pub(crate) fn collection(&self, rk: &str) -> CollectionOutcome {
        let Some(guid) = Self::guid(rk) else { return CollectionOutcome::Missing };
        let Some(uid) = self.user_id() else { return CollectionOutcome::Transport };
        let (st, it) = self.get_status::<BaseItemDto>(&Q::new(format!("/Items/{guid}")).s("userId", &uid).build());
        collection_outcome(st, it.map(|i| convert::container(vec![convert::item(&i, 0)], 1, 0)))
    }

    pub(crate) fn collection_children(&self, rk: &str, start: i64, size: i64) -> CollectionOutcome {
        let Some(guid) = Self::guid(rk) else { return CollectionOutcome::Missing };
        let mut q = Q::new("/Items").s("ParentId", &guid).s("Fields", LIST_FIELDS)
            .s("SortBy", "PremiereDate,SortName").b("EnableTotalRecordCount", true).i("StartIndex", start);
        if size > 0 {
            q = q.i("Limit", size);
        }
        self.collection_page(q, 0)
    }

    fn collection_page(&self, q: Q, section: i64) -> CollectionOutcome {
        let q = match self.user_id() {
            Some(uid) => q.s("userId", &uid),
            None => return CollectionOutcome::Transport,
        };
        let (st, r) = self.get_status::<QueryResult<BaseItemDto>>(&q.build());
        collection_outcome(st, r.map(|r| {
            let rows = r.items.iter().map(|i| convert::item(i, section)).collect();
            convert::container(rows, r.total_record_count, r.start_index)
        }))
    }

    // ---- subtitles ------------------------------------------------------------------------

    /// An external subtitle stream (the key [`convert::stream`] built) as UTF-8 text; ASS/SSA keep
    /// their own format, everything else is asked for as SubRip.
    pub fn sidecar_subtitle(&self, key: &str, codec: &str) -> Option<Vec<u8>> {
        if !key.starts_with("/Videos/") || !key.contains("/Subtitles/") {
            return None;
        }
        let path = if codec.eq_ignore_ascii_case("ass") || codec.eq_ignore_ascii_case("ssa") {
            key.to_string()
        } else {
            match key.rsplit_once("/Stream.") {
                Some((head, _)) => format!("{head}/Stream.srt"),
                None => key.to_string(),
            }
        };
        let r = self.send(&path, Method::Get, None)?;
        (r.ok() && !r.body.is_empty() && r.body.len() <= crate::plex::SIDECAR_MAX_BYTES).then_some(r.body)
    }
}

fn collection_outcome(status: Option<i32>, page: Option<MediaContainer>) -> CollectionOutcome {
    match (status, page) {
        (Some(401 | 403), _) => CollectionOutcome::Denied,
        (Some(404), _) => CollectionOutcome::Missing,
        (Some(200..=299), Some(p)) => CollectionOutcome::Ok(p),
        _ => CollectionOutcome::Transport,
    }
}

fn provider_key(scheme: &str) -> &'static str {
    match scheme {
        "imdb" => "Imdb",
        "tmdb" => "Tmdb",
        _ => "Tvdb",
    }
}

fn parse<T: DeserializeOwned>(path: &str, body: &[u8]) -> Option<T> {
    match serde_json::from_slice::<T>(body) {
        Ok(v) => Some(v),
        Err(e) => {
            plx_base::eventlog::log(&format!(
                "jf: GET {} answered {} bytes that will not parse — {e}",
                path.split('?').next().unwrap_or(path),
                body.len()
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plex_sort_expressions_translate_to_items_sorts() {
        assert_eq!(sort_by("titleSort:asc"), ("SortName".to_string(), "Ascending"));
        assert_eq!(sort_by("addedAt:desc"), ("DateCreated,SortName".to_string(), "Descending"));
        assert_eq!(sort_by("showOrder:desc").0, "SeriesSortName,ParentIndexNumber,IndexNumber,SortName");
        assert_eq!(sort_by("random").0, "Random");
        assert_eq!(sort_by("").0, "SortName");
        assert_eq!(sort_by("nonsense:desc"), ("SortName".to_string(), "Descending"));
    }

    #[test]
    fn the_query_builder_encodes_values_and_skips_empty_ones() {
        assert_eq!(Q::new("/Items").s("searchTerm", "tom & jerry").s("ParentId", "").i("Limit", 5).build(),
            "/Items?searchTerm=tom%20%26%20jerry&Limit=5");
        assert_eq!(Q::new("/x?a=1").b("Recursive", true).build(), "/x?a=1&Recursive=true");
    }

    #[test]
    fn every_advertised_sort_is_one_the_translator_knows() {
        for t in ["Movie", "Series", "Episode", "Season"] {
            for s in &sort_menu(t).types[0].sort {
                let (fields, _) = sort_by(&format!("{}:asc", s.key));
                assert!(fields != "SortName" || s.key == "titleSort", "{} unmapped", s.key);
            }
        }
    }
}
