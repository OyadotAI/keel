# Keel launch kit

One-liner: **Love Claude Code. Hate the CLI and VS Code extension experience? This is for you.**

Category: an opinionated **agentic development environment (ADE)** for developers and product
engineers who love Claude's agent but want a better experience around it.

Commitment: **Free. Open source. Always.** Keel has no subscription or license fee; Claude
account access and other provider usage are separate.

Founder-provided usage statement: **Used daily by engineers at Oya.ai and Jumpermedia.co.**
Keep that wording precise. It is not a claim about company-wide adoption, user counts, or an
endorsement by either company.

Sponsor: **[getOya.ai](https://getoya.ai), a runtime for accurate agents.**
Lead maintainer: **Mohamed Alkiswani** ([mk@getoya.ai](mailto:mk@getoya.ai)).

The founder story has three parts: keeping a team's skills, plugins, hooks, and tools consistent;
seeing what agents do and which files each session changes; and keeping individual Claude accounts
while developing shared workflows. Lead with those problems, then show the product answering them.

Key differentiator: **Your old sessions. Your live agents. One workspace.** Keel discovers existing
project conversations and follows running sessions, not just work started inside Keel. CLI,
VS Code, Orca, and other front-ends are relevant when they write compatible, discoverable local
Claude Code transcripts. This is same-Mac visibility, not cloud account history synchronization.

The current proof is guided per-Mac installation, native configuration surfaces, parallel task
tabs, explicit approvals, and visible review evidence. Do not imply automatic fleet-wide setup
sync, central policy enforcement, or an alternative to Anthropic's billing and organization
administration. Describe subscription preferences as the founder's motivation, not a claim about
current plan restrictions. “10× better” is an ambition, not a measured result.

## Assets

All assets are captured from a real run of the desktop app (0.3.13) against a small demo
repository, `storefront`. The turns, approvals, test results, durations and costs on screen are what
that run actually produced. The demos are sped up; nothing in them is scripted.

| Asset | Use |
| --- | --- |
| [Workspace screenshot](media/workspace.png) | README hero, product overview |
| [Workspace demo](media/workflow.mp4) · [GIF](media/workflow.gif) | Main social post; full task-to-review loop |
| [Turn evidence demo](media/turn.mp4) · [GIF](media/turn.gif) | Explain what Keel adds around the agent |
| [Approval demo](media/approve.mp4) · [GIF](media/approve.gif) | Show exact commands and permission scope |
| [Command palette](media/commands.png) | Keyboard-first workflow |
| [Agent team](media/team.png) | pm, principal, reviewer, security and qa running one request |
| [Add the team](media/readiness.png) | Readiness offering the agent team for an existing repository |

Use MP4 for social uploads and GIF for Markdown. Videos are silent, H.264-encoded and
web-playback optimized.

To recapture after UI changes, run the installed app with its own WebKit profile so your real
projects stay out of frame, open a demo repository, and record the window with ffmpeg:

```sh
CFFIXED_USER_HOME=/tmp/keel-demo-home /Applications/Keel.app/Contents/MacOS/keel-desktop
```

Keep the window at 1600×1000. GIFs are 1200px wide at 10 fps with a 128-colour palette.

## Short launch post

> Love Claude Code. Hate the CLI and VS Code extension experience?
>
> I built Keel: an opinionated ADE for developers and product engineers.
>
> Bring your old Claude sessions. Watch running agents live—even when they started elsewhere.
>
> An agent team in every project. Your Claude account.
>
> Native macOS. Free and open source, always.

Attach `workflow.mp4`; link the public project/download page when the build containing these
features is published. Label the clip “scripted UI demo” in the post or its caption.

## Technical-community launch

Suggested title: **Show HN: Keel — an opinionated native workspace for Claude Code**

Suggested opening:

> I love Claude Code's agent. I don't love the CLI or VS Code extension experience.
>
> On my team, getting everyone onto the same skills, plugins, hooks, and tools meant constantly
> chasing missing setup. During sessions, I also struggled to see what agents were doing and
> which files they had changed. I wanted us to share a way of working while keeping our
> individual Claude subscriptions.
>
> So I built Keel, an opinionated ADE for developers and product engineers. It guides CLI
> installation and sign-in, exposes agent configuration in native panels, and keeps changed
> files, commands, checks, and approvals beside the conversation. Parallel tasks have their
> own tabs and can use isolated Git worktrees.
>
> Engineers at Oya.ai and Jumpermedia.co use it daily. Keel is free and open source, and will
> always be free to use. Claude account access is separate.
> The project is sponsored by getOya.ai, a runtime for accurate agents.
>
> You don't need to start over: Keel loads existing project conversations, shows their recorded
> details, and follows running sessions in real time. That includes compatible local Claude Code
> sessions launched from the CLI, VS Code, Orca, or another front-end—not just Keel's own tasks.
>
> The app is Tauri and React; a local Rust daemon owns agent processes, Git, and terminals.
> It drives the installed provider CLI. It is not a new model or an Anthropic product.
>
> I'd especially like feedback on first-run setup, keyboard navigation, and reviewing several
> tasks at once.

## Before publishing

- Put these community files on the default branch so GitHub discovers the issue and PR templates.
- Enable Issues and private vulnerability reporting in the source repository's settings.
  The private reporting fallback is `mk@getoya.ai`.
- Configure main-branch protection and required checks. Add CODEOWNERS only after choosing real
  reviewer handles or teams; do not invent owners or imply these settings are enforced by a template.
- Publish a signed build that includes the advertised features. This working-tree update alone
  does not update the public download.
- Run real first-install, browser-login, and provider acceptance on a clean macOS account.
- Recheck the README on GitHub at desktop and narrow widths; keep demo captions accurate.
- Verify every download link and the MP4 playback on the destination platform.
- Keep privacy precise: provider requests can send prompts and code off-machine, and configured
  builds enable crash/usage reporting by default. Link the [privacy guide](privacy.md).
- Do not add invented testimonials, customer counts, performance multipliers, or benchmark badges.

No launch posts or releases were published as part of preparing this kit.
