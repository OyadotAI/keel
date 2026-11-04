<!-- Development and hosting boundaries for Oya's standalone product-family landing page. -->
# oya.dev

Oya's product directory for Keel, Oya Browser, and Keel Reviewer. The entire deployable artifact is
`website/public/`: static HTML, CSS, and local assets. It needs no installation, JavaScript, build
step, environment variables, or backend.

From the repository root, preview with:

```sh
python3 -m http.server 4173 --bind 127.0.0.1 --directory website/public
```

Open `http://127.0.0.1:4173`. Stop the server with Ctrl+C. Serve this public directory, never the
repository root: the repository contains local configuration and private development state.

Run the hermetic page checks with `make website-test`; they also run in `make check` and CI.
They verify product destinations, fragment navigation, local asset containment and availability,
image alternatives, and the document's no-script structure. They do not replace rendered layout,
real keyboard, or responsive browser testing.

## Content and assets

- The theme follows Oya Browser's **public landing page**, using its white canvas, DM Sans,
  green `#16733b`, pale-lime `#c7f7a4` actions, and forest `#1b2d22` panels. These come from
  `agentchrome/ui/src/app/page.module.css`, including its final layout overrides. The app's
  dark globals and Archivo display font do not apply to that public page.
- `public/assets/oya-logo.svg` preserves the original two-ring vector from
  `agentchrome/ui/src/components/oya-logo.tsx`; its strokes resolve the public page's primary
  and foreground colors. Header and footer share this asset. The favicon preserves the
  supplied `agentchrome/ui/src/app/icon.svg` geometry and colors. Brand source was checked
  against the local Oya Browser checkout and the live site through Oya on 2026-11-03.
- Keel's screenshot is copied, unaltered, from `docs/media/workspace.png`. It is an existing demo
  workspace, not a claim that the picture shows the latest release.
- DM Sans is copied from `desktop/src/fonts/dm-sans-latin.woff2`; its original SIL Open Font
  License is retained in `public/assets/OFL.txt`.
- Browser and Reviewer diagrams are HTML/CSS illustrations, explicitly labeled as such.
- Browser copy follows [Oya Browser](https://oyabrowser.com), checked through Oya on 2026-11-03.
- Keel and Reviewer copy follows this repository, its current license, and
  `packaging/action/review/README.md`. Source, release assets, and public Reviewer documentation
  were checked through Oya on 2026-11-03. External links can change after that check.

Shared values live in `tokens.css`. Layouts live in `styles.css`; diagram styles are kept in
`illustrations.css` to keep the files small. The single semantic HTML document intentionally
avoids a template/build system. Essential content and navigation work with JavaScript disabled.

The page is prepared for `https://oya.dev/` (canonical and social metadata). Hosting, deployment,
DNS changes, and individual product sites are outside this change. Publish only `website/public/`
when deployment is authorized. There are no new network services or analytics in the page.
