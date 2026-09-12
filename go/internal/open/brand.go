package open

import (
	_ "embed"
	"encoding/base64"
)

// These assets are copied from assets/brand by npm run build:viewer.
//
//go:embed branding/mark.svg
var brandSVG string

//go:embed branding/favicon.svg
var faviconSVG []byte

//go:embed branding/brand.css
var brandCSS string

var favicon = `<link rel="icon" type="image/svg+xml" href="data:image/svg+xml;base64,` + base64.StdEncoding.EncodeToString(faviconSVG) + `">`

var brandHeader = `<header class="viewer-brand"><a class="slink-brand slink-brand--compact" href="/" aria-label="session.link home">` + brandSVG + `<span class="slink-brand-name">session<span class="slink-brand-dot">.</span>link</span></a><a href="/settings" style="margin-left:auto;font-size:12px">Recovery and devices</a></header>`
