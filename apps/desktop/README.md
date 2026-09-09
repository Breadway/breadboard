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

Needs the Tauri Linux deps (`webkit2gtk-4.1`, `libsoup3`, …) and an icon at
`icons/icon.png` (`npx tauri icon <src.png>` generates the set). Not in the
Cargo workspace or CI; build it on demand:

```sh
cd apps/desktop
npm install
npm run dev      # or: npm run build  → AppImage
```

Distribute via bakery alongside `breadboard-cli` once release secrets are set up.
