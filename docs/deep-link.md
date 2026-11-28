# Opening an item or a user from a launcher (deep link)

A launcher can open Native Jelly on one item's detail page, as one of the users already kept on
the television, by passing launch parameters:

```sh
luna-send -n 1 luna://com.webos.applicationManager/launch \
  '{"id":"com.sostk.nativejelly","params":{"userId":"<user id>","itemId":"<item id>","serverId":"<server id>"}}'
```

| key | meaning |
| --- | --- |
| `itemId` | Open this item's detail page (never playback). |
| `userId` | Be this user. Picked from the users already signed in on this television, like a tile on the who's-watching screen; the screen is skipped. |
| `serverId` | Optional. Dropped when it names another server than the user's (or the signed-in one). |

All ids are Jellyfin GUIDs, dashed or not. At least one of `itemId` and `userId` is needed; other
keys (`tmdbId`, `mediaType`, `title`) are ignored.

- webOS passes `params` to a native app as one JSON string in `argv[1]`; `src/main.c` hands it to
  `app::deep_link`.
- `userId` only chooses among kept users. It never signs anyone in, takes no password or token,
  and a user who is not kept is dropped, not guessed. Jellyfin users have no PIN here, so this
  is what the remote could do. A kept user whose sign-in the server has since revoked lands on
  the same error a pick from the who's-watching screen would.
- The link waits for Home (behind sign-in, the who's-watching screen or a first-run question)
  and is dropped after 45 seconds.
- Anything that is not a GUID is refused before it reaches the catalog or the log.
- A launch while the app is running arrives as SAM's `relaunch` event. LG's SDL would drop its
  `parameters`, so `src/main.c` subscribes to `registerApp` itself (through `libhelpers`, as SDL
  does) and sets `SDL_HINT_WEBOS_REGISTER_APP` so SDL skips its own; the link then waits for Home
  like a cold launch. On webOS 6 and later `argv[1]` is the same lifecycle message, with the
  params under `parameters`.

CleanYourTV's *Jellyfin continue watching* add-on sends `itemId` and `serverId`.
