# breadboard desktop

A thin Tauri 2 shell around <https://breadboard.breadway.dev>. The window loads
the live site directly, so session cookies and the Forgejo OAuth redirect work
with no extra auth plumbing.

Global shortcuts:

| | |
|---|---|
| `Ctrl+Shift+B` | show / hide the window |
| `Ctrl+Shift+N` | show + open the "new issue" palette (`#new` hash) |

## Build

Needs the Tauri Linux deps (`webkit2gtk-4.1`, `libsoup3`, GTK 3). Not in the
Cargo workspace or CI; build it on demand:

```sh
cd apps/desktop
npm install
npm run dev                # dev shell against the live site
npm run build              # release bundle -> AppImage + .deb under
                           # target/release/bundle/
```

## Bundle

`tauri.conf.json` carries the full bundle metadata: identifier
`dev.breadway.breadboard`, product name `breadboard`, `DeveloperTool` category,
copyright, publisher, homepage, and MIT license. Linux deb/rpm dependencies are
detected by Tauri from the linked libraries. With no `version` key in
`tauri.conf.json`, Tauri takes the bundle version from this crate's
`Cargo.toml`, which tracks the workspace `0.1.0`; bump them together.

Verified locally: `cargo tauri build --bundles deb` produces
`target/release/bundle/deb/breadboard_0.1.0_amd64.deb` (correct control fields,
hicolor icons, `Categories=Development` desktop entry). The AppImage target
compiles and links the same binary but its bundling step needs `linuxdeploy`
(downloaded from GitHub, run via FUSE), which a sandboxed CI runner may block;
run `npm run build` on a real host for the AppImage.

The app icon (`icons/`, the bread mark on a blue field, matching the web
favicon) is the standard Tauri set. Regenerate it from a square source with:

```sh
cargo tauri icon path/to/source.png   # then delete icons/android, icons/ios
```

The gated bakery workflows build the deb bundle and distribute its native
binary as `breadboard-desktop`, alongside the separate `breadboard-cli`
package. See `packaging/BAKERY.md` at the repository root for activation and
signing requirements. Neither building nor staging starts the desktop app.
