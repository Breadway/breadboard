# GitHub mirror

Forgejo is authoritative. Contributors push branches to `origin` only. The
`mirror.yml` workflow publishes main and version tags to
`https://github.com/Breadway/breadboard` after origin receives them. It does not
publish work branches or delete destination refs. A divergent destination
fails without forcing history replacement.

Activation requires a GitHub repository owned by Breadway and a valid
`MIRROR_TOKEN` repository Actions secret with access to that repository. Use a
repository-scoped token with content write permission and workflow write
permission because this repository includes workflow files. Do not reuse the
previous compromised or broken ecosystem mirror token. The token is passed
through an ephemeral askpass helper, never placed in a remote URL.

The workflow is disabled until the repository Actions variable
`BREADBOARD_MIRROR_ENABLED` is set to `true`. Create or verify the GitHub
destination and provision the valid secret before enabling it, then dispatch
the workflow. Confirm the main SHA and version tags match origin and inspect
the workflow result before considering the mirror active. Disable the variable
to stop automatic updates; no repository or credential removal is needed.

During preparation on 2026-09-30, the destination returned HTTP 404 without
authentication, the Forgejo push-mirror list was empty, and this repository had
no Actions secrets. A private destination may also return 404, so its existence
must be checked by the owning GitHub account during activation.
