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
- Keel's screenshot is the unaltered image supplied by the user on 2026-11-04, stored at
  `public/assets/keel-workspace.png`. It shows a live agent terminal and embedded browser.
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

## Cloudflare Pages

The published project is `oya-dev`, with production branch `main` and Pages hostname
<https://oya-dev-2fs.pages.dev>. The production custom domain is <https://oya.dev/>; its canonical
and social metadata already use that address. Publish only `website/public/`.

From the repository root, using an authenticated Cloudflare account:

```sh
make website-test
wrangler whoami
wrangler pages deploy website/public --project-name oya-dev --branch main
```

Wrangler reports the immutable deployment URL after upload. Check the production hostname and
custom domain in Oya after deployment. The project was first published from commit `b963302`;
the immutable initial deployment is <https://a09e45f0.oya-dev-2fs.pages.dev>.

The apex domain belongs to an active zone in the same Cloudflare account. Its Pages custom-domain
binding is registered on `oya-dev`, and its apex CNAME targets `oya-dev-2fs.pages.dev` (proxied,
automatic TTL). The custom-domain page and its product navigation were verified over HTTPS in
Oya on 2026-11-03. Email records were preserved during the cutover.

Keep email, verification, and subdomain records when changing the web record. Inspect Pages'
custom-domain and certificate status after a change; DNS verification and final certificate
validation can have separate states. Record the previous apex web target before replacing it
so the DNS change can be reversed. If Wrangler cannot edit DNS with its current token scopes,
use the signed-in Cloudflare Pages domain wizard to review and activate the proposed record.

To roll back site content, select a previous production deployment in the Pages project; preserve
the DNS and custom-domain binding. There are no new backend services or analytics in this page.
