# session.link identity

The adopted mark is **Elision**: brackets around three dots, the editorial
sign for an excerpt. Pair it with the compact sans wordmark in `brand.css`.
It keeps the brackets, stroke and proportions of the original Excerpt mark
(two lines and a dot), which it replaced on 2026-10-07. The diagonal-arrow
explorations were not selected.

`mark.svg` uses the surrounding `--signal` color when embedded inline, including
the local viewer's dark theme. `favicon.svg` adapts to light and dark browser
chrome. Keep the geometry identical between these two assets.

`npm run build:viewer` copies this directory's CSS and icons into the Go
viewer's embedded assets. The server checkout keeps matching copies under
`public/branding/`, uses the favicon as `app/icon.svg`, and imports the same
CSS for the hosted viewer and landing page.
