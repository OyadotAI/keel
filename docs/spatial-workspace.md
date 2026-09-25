# Spatial workspace

Keel opens onto the conversation. Sessions, Changes, Files, and Git share a labeled sidebar; Tools & agents holds the remaining project destinations. The existing command palette and menu shortcuts still reach every destination.

## Inspector

Review, activity, and selected files share one inspector. New installations start with it closed. Visibility is remembered for the next window, while each open window controls its own presentation. Selecting a file again reopens a closed inspector. Expanding inspection preserves the mounted conversation and its draft.

- **⌘⌥I:** show or hide the inspector.
- **⌘⌥⇧I:** expand or restore inspection.
- **⌘L:** return to the conversation and focus its composer.
- **⌘⇧E:** show or hide the sidebar.
- **⌘⌥T:** show or hide the terminal.

Below 1280 points, navigation becomes a drawer. Below 900 points, inspection becomes a full-width destination with a visible Conversation action. The sidebar is available as a dismissible drawer when it cannot fit beside the content. Stored pane widths are clamped for display without being overwritten.

## Visual system

System typography, warm paper/charcoal surfaces, and Oya green support both appearances. The 52-point command band combines tabs and workspace actions. Connection, branch, checks, permissions, and usage live in its status popover; the terminal remains one click away. The default sidebar is 320 points and inspector 420 points, with existing saved widths clamped to a 300–420 point sidebar range. Native materials are confined to navigation and the rounded inspector shell. Conversation, code, terminal, and diff surfaces remain opaque. Reduced transparency uses solid chrome; reduced motion suppresses workspace transitions and button scaling. Semantic amber, red, and green retain their status meanings.

## Verification

The visual catalog includes focused, split, and expanded workspaces in both appearances at 600, 900, 1100, and 1512 points. Layout tests cover width constraints and restoration; interaction tests exercise real keyboard notification handlers, draft retention, repeated inspector navigation, and terminal visibility. Existing contrast, rendering, streaming, scrolling, approval, and layout-budget tests remain part of the Swift suite.

Run `KEEL_VISUAL_CATALOG=/private/tmp/keel-catalog swift test --package-path app --filter VisualCatalogTests` to regenerate the screenshots. `make app-test` builds the distributable and runs the Swift suite.

## Composer and panel workflows

The composer uses an AppKit text view for native selection, undo, input methods and paste at the cursor. Return inserts a selected completion before it can send; Shift-Return inserts a newline. Add opens file attachments, searchable project context and searchable commands. Expand provides a 260-point editing area. Attachment chips open previews, and queued messages can be returned to the draft without overwriting existing text. Ordinary typing no longer files the whole draft automatically; long clipboard pastes still become text attachments.

Panels keep their title, purpose and primary action above the scrolling content. Compact navigation leaves more height for lists. Skills, subagents, servers and plugins have search; file search reports empty and truncated results. Git exposes Fetch, Pull and Push, with a separate commit message area and explicit commit scope. Stage/unstage and discard remain in More. Background output has a dedicated disclosure so selecting logs and stopping a job do not compete with a row-wide click target.

Live messages settle into position; tool and reasoning disclosures use the shared spring. Streaming remains coalesced in block-level views and is not animated character by character. All new motion respects Reduce Motion.

Streaming responses use one native eased tail follower, retargeted continuously as laid-out content grows. Manual scroll phases cancel it immediately; Reduce Motion follows without interpolation. The agent avatar adapts AgentChrome’s companion orb: a shaded lens, accent halo and orbiting ring, shown both in response headers and the live activity bar. Only the small avatar ticks; historical avatars are still.
