# Product screenshots

These are browser captures of the current local product, built from the working
tree at `2e3d79b` with the Elision mark, the adopted identity. The interface is
captured as rendered by the product, at a device scale factor of 2, so each
file has twice the pixel dimensions listed below and stays sharp on HiDPI
screens. Sizes are CSS pixels; the landing page declares the same sizes. The sessions are fictional examples
from [`testdata/landing/sessions.mjs`](../../testdata/landing/sessions.mjs), not
private transcripts or claims about real companies.

| Asset | Size | Product state |
| --- | --- | --- |
| `focused.webp` | 1280 × 940 | Latest human input and answer, scoped search, compact navigation, collapsed activity |
| `focused-mobile.webp` | 390 × 740 | The same reading view at phone width, cropped to about one phone screen |
| `sessions.webp` | 1100 × 680 | Searchable project list with individual cards and day groups |
| `search.webp` | 1280 × 940 | Whole-session search, including earlier answers and agent activity |
| `agent-activity.webp` | 1280 × 1200 | Recorded command and test output expanded beneath the answer |
| `prepare-view.webp` | 1100 × 960 | Title, author comment, explicit inclusion, preview, and local save |

Use `focused.webp` as the landing hero, with `focused-mobile.webp` for narrow
screens. Link screenshots to their full-size assets. Label the examples as
fictional. The view preparation screenshot shows the current local-only save
and download flow; it must not be presented as hosted excerpt publishing.
Keep the hero's declared width and height in CSS pixels; the 2x files scale down
in the browser.
The landing page's live example is `sessions[0]` from the same fixture file,
copied to the server repo as `public/demo/session.json`; keep the two in step.

## Regenerate

Requires Node 22+, the repository dependencies, Go 1.26+, and a Chromium browser.
Build the viewer and CLI from the same checkout before capturing:

```sh
npm run build:viewer
cd go
go build -o /tmp/session-link-landing-slink ./cmd/slink
cd ..
node scripts/capture-product.mjs
```

`SLINK_BINARY` and `BROWSER_BINARY` override the binary locations.
`SCREENSHOT_DIR` overrides this output folder, so the same captures can be
written directly to the server repo's `public/product` directory. The script
uses an isolated local home and fictional captures, checks the source format
and key UI states, and closes the viewer and browser when finished.

The landing page itself lives in the separate `lftherios/session-link-server`
repository, in `public/landing.html`. `capture.json` records the capture date
and fixture source. Screenshots are WebP, captured directly by the browser.
