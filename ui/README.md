# Keel UI

React + Vite, built to `dist/` and embedded into the `keel` binary with `rust-embed`. Served from
`http://127.0.0.1:7777`; it is never exposed off-host.

## The three panes

This is a ship console, not an editor. There is a CodeMirror pane for reading and quick edits, but
editing is not what the product is for — you keep your own editor open on the same directory.

- **GitHub** — branch, open PRs, CI checks, what the agent changed
- **Dev / Prod** — deployed version, error rate, p95, invocations, with the **Promote** button
  between them showing the drift ("dev is 3 commits ahead")
- **Preview** — the current version's URL, live

## The composer's environment toggle is a guardrail

The dev/prod switch in the chat composer is not decoration — it decides whether the agent acts
freely or needs approval for every mutation. Making the target environment visible where the user
types is the clearest trust affordance in the product.
