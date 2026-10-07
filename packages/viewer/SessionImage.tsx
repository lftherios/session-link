// Transcript URLs are untrusted and must never trigger a request just by
// opening a session. Embedded raster images stay local; external ones require
// an explicit navigation, without a referrer or access to the opening page.
export function SessionImage({ src, alt = "Image" }: { src?: string; alt?: string }) {
  if (src && /^data:image\/(?:png|jpe?g|gif|webp|avif);base64,[a-z0-9+/=\s]+$/i.test(src)) {
    return <img src={src} alt={alt} loading="lazy" style={{ maxWidth: "100%", height: "auto" }} />;
  }
  if (src && /^https?:\/\//i.test(src)) {
    return <span>{alt || "External image"} · <a href={src} target="_blank" rel="noopener noreferrer">Open external image</a></span>;
  }
  return <span>{alt || "Image unavailable"}</span>;
}
