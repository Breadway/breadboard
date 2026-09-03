# breadboard desktop (Tauri 2)

Milestone 6. A thin Tauri 2 shell that wraps `apps/web`'s static build — the
same stack as `bos-settings` (Tauri 2 + Svelte, Rust backend dir at `src/`,
frontend built to `../web/dist` and referenced as `frontendDist` in
`tauri.conf.json`).

Adds over the web app:

- a global quick-add hotkey → create an issue from anywhere
- offline is **not** in scope for v1 (online-required, optimistic UI)

To scaffold when the milestone starts:

```sh
cd apps/desktop
npm create tauri-app@latest -- --template vanilla-ts   # then rewire to ../web
```

Distributed through bakery as part of the `breadboard` package (see
`bakery.toml` / `.forgejo/workflows/`).
