# session.link identity

The adopted mark is **Elision**: brackets around three dots, the editorial
sign for an excerpt. Pair it with the compact sans wordmark in `brand.css`.
It keeps the brackets, stroke and proportions of the original Excerpt mark
(two lines and a dot), which it replaced on 2026-10-07. The diagonal-arrow
explorations were not selected.

The brand colors are cobalt `#1f44ff` for the mark and the wordmark's dot and
navy `#0a1033` for the wordmark, on cool white `#f6f8ff`. On dark surfaces the
cobalt lightens to `#7d9bff` over navy `#0b1030`. The viewer and the landing
page share these values; the viewer's full token list is in
`packages/viewer/RunViewer.tsx`.

`mark.svg` uses the surrounding `--signal` color when embedded inline, including
the local viewer's dark theme. `favicon.svg` adapts to light and dark browser
chrome. Keep the geometry identical between these two assets.

`npm run build:viewer` copies this directory's CSS and icons into the Go
viewer's embedded assets. The server checkout keeps matching copies under
`public/branding/`, uses the favicon as `app/icon.svg`, and imports the same
CSS for the hosted viewer and landing page.
