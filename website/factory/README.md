<!-- Hosting for oyafactory.com, Oya Factory's own landing page. -->
# oyafactory.com

A static page: `public/` is the whole deployable artifact (HTML, one stylesheet, local assets), with
no script, build step or analytics. Dark only, in the five Oya colours, like the app.

Preview from the repository root:

```sh
python3 -m http.server 4174 --bind 127.0.0.1 --directory website/factory/public
```

Publish to the Cloudflare Pages project `oya-factory` (custom domain `oyafactory.com`):

```sh
wrangler pages deploy website/factory/public --project-name oya-factory --branch main
```

Every claim on the page is something the shipped app does; check it against `CLAUDE.md` before
adding one. The screenshot is shared with oya.dev; see `website/README.md` for how it is made.
