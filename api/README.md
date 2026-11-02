# Public API contract

Other projects depend on the 4K Cam Manager API: Cockpit (widgets, actions and joystick
suggestions from `cockpit_extras.json`), BlueOS (`register_service`), and third-party
tools calling `/v1/camera/control` and `/v1/autopilot/control`. Cockpit also **saves
the request body of each action** in user configurations, so those bodies have to keep
working across upgrades, even after we stop generating them.

This directory pins that API with [insta](https://insta.rs) snapshot tests, the same
technique [rust-mavlink](https://github.com/mavlink/rust-mavlink) uses for its generated
bindings: fixed inputs go through the real code, and the outputs are committed as
snapshots. `cargo test` fails when the code no longer matches them (see
`backend/libs/br4kcam_manager/tests/api_snapshots.rs`).

| Path | What it pins | Who edits it |
| --- | --- | --- |
| `endpoints.txt` | HTTP routes, WebSocket requests and WebSocket events that must keep existing | By hand, **append-only** |
| `requests/<kind>/*.json` | Request bodies clients send, which must keep being accepted with no field dropped | By hand, **append-only** |
| `snapshots/<kind>_requests@*.snap` | How the backend understands each recorded request | insta |
| `snapshots/types_*.snap` | The wire types of requests, responses and WebSocket messages | insta |
| `snapshots/{cockpit_extras,register_service,info}.snap` | The documents Cockpit and BlueOS fetch | insta |

The `types_*` snapshots come from ts-rs, so they inherit its view of serde: optional
fields that are omitted on the wire show up as `| null`. Read them as a change detector,
not a specification. Enums the camera protocol sends as numbers are shown as their
numeric values.

## Changing the API

**Backwards-compatible changes are fine**, but they need a deliberate snapshot update:

- adding a route, a WebSocket request or event, or a control `action`;
- adding a field to a response or event;
- adding an *optional* field to a request;
- adding an enum variant that clients only receive, never have to send.

Add new endpoints to `endpoints.txt` and a recorded request for any new request shape
under `requests/`, then review the new snapshots and commit them with the change:

```bash
cargo insta test -p br4kcam_manager --test api_snapshots --review
# or, without cargo-insta:
INSTA_UPDATE=always cargo test -p br4kcam_manager --test api_snapshots
```

In CI, insta never writes snapshots; a mismatch or a missing snapshot fails the build.
Pull requests that change this directory need the `api-change` label.

**Breaking changes are not allowed in `/v1`.** These include removing or renaming a
route, field, enum variant or `action`; changing a field's type or an enum's wire value;
making a request field required; and changing a Cockpit action, widget or joystick
suggestion `id`. Add the new form alongside the old one, or introduce `/v2` and keep
serving `/v1`. If a break is really unavoidable, it needs the `breaking-api-change`
label, which is the only way CI accepts edits or removals in `endpoints.txt` and
`requests/`.
