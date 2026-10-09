# Several people on one Jellyfin server

Native Jelly keeps up to twelve Jellyfin users signed in on one television and asks *Who's
watching?* when more than one is kept. This is the map of where that lives; each module's own doc
comment has the detail.

## What is stored (`rust-modules/src/jf/store.rs`)

The Jellyfin record in the session file (`session::set_jellyfin_sign_in`, with a 0600 file as the
fallback) is a **roster**, version 2:

- every kept user (`users`, newest first, at most `MAX_USERS` = 12): server, token, user id and
  name, server id and name, and the DeviceId basis the token was minted under;
- the **active** user's fields again at the top level, exactly where a version-1 record kept them,
  so an older build still finds a working sign-in;
- `recent_server`: the server signed in to last (address, name, id; never a user or a token), which
  outlives every sign-out and fills the sign-in screen's *Recent* row.

A version-1 record reads as a roster of one. With nobody signed in the record is
`{"version":2,"recent_server":{…}}`, which no build reads as a sign-in. Only Delete all local data
removes it.

## Whose history is whose (`catalog::session::current_profile_key`)

Pins (Home library choices), recent searches, the last library, sort choices and subtitle offsets
are filed under the profile key, which is `jf-<user id>` for the active Jellyfin user. `jf::store`
publishes it on every roster change. An install upgraded from one user kept that history under the
empty key; boot gives it to the active user once (`session::adopt_unscoped_profile`), list by list,
never over a list that user already has. Telemetry consent and the playback, language and
Automatically Sign In preferences belong to the whole television.

## The flows

| Where | What happens | Code |
| --- | --- | --- |
| Boot, more than one user kept | Installed as the last active user, landing on *Who's watching?* — unless Automatically Sign In is on | `app/boot.rs` |
| *Who's watching?* pick | The screen asks the app to check the user's token (`JfAuthCmd::CheckKept`, `GET /Users/Me`); refused → *Sign in as name* again, otherwise `PickJellyfinUser` | `screens/jf_users.rs` |
| Pick the user already active | Straight to their Home (or first-run Favourites) | `app/jf_login.rs::pick_user` |
| Pick another user | Active user moves, clients revoked, server installed again under their token and DeviceId through the sign-in handoff (`bridge::follow_auth_landing`), every page of the previous user dropped | `app/jf_login.rs::pick_user` |
| Account menu → Add user | Sign-in screen opened as *Add a user* (`Opening::AddUser`): the server's `GET /Users/Public` list, *Other user*, Quick Connect | `screens/jf_login.rs` |
| Account menu → Sign out of name | Only that user is forgotten and their token revoked; *Who's watching?* if anyone is left, else the sign-in screen with *Recent* | `app/jf_login.rs::sign_out` |
| Delete all local data | Every user forgotten, every token revoked, the record and *Recent* erased | `app/jf_login.rs::sign_out_and_forget_server` |

Screens never hold a token: *Who's watching?* sends a roster position and the loop reads the roster
again before acting.

## Testing it in the simulator

A mock server with several users (`/Users/Public`, per-user tokens from `AuthenticateByName`) is
enough; drive the flows with the remote FIFO as the `ui-sim` skill describes. To check that an
expired sign-in is asked for again rather than shown as an unreachable server, have the mock answer
401 to one user's token.
