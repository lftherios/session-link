import { Children, isValidElement, memo, useId, useState, type ReactNode } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

export const DOCUMENT_CSS = `
.rv .rv-document{font-size:14px;line-height:1.75;overflow-wrap:anywhere;min-width:0}
.rv-document>:first-child{margin-top:0}.rv-document>:last-child{margin-bottom:0}
.rv-document p{margin:12px 0;white-space:pre-wrap}.rv-document h1,.rv-document h2,.rv-document h3,.rv-document h4{font-family:inherit;font-weight:600;letter-spacing:normal;line-height:1.35;margin:24px 0 12px}
.rv-document h1{font-size:24px}.rv-document h2{font-size:21px}.rv-document h3{font-size:17px}.rv-document h4{font-size:15px}
.rv-document a{color:var(--rv-signal)}.rv-document ul,.rv-document ol{padding-left:24px;margin:12px 0}.rv-document li{margin:5px 0}.rv-document li>p{margin:6px 0}
.rv-document blockquote{border-left:3px solid var(--rv-line);padding-left:16px;margin:16px 0;color:var(--rv-faint)}
.rv-document code{font: .9em ui-monospace,monospace;background:var(--rv-soft);padding:2px 4px;border-radius:4px}
.rv-document pre{white-space:pre;overflow:auto;padding:15px!important;border-radius:6px;background:var(--rv-soft);font:12px/1.6 ui-monospace,monospace}
.rv-document pre code{padding:0;background:transparent;font:inherit}.rv-document .rv-code{margin:14px 0}.rv-document .rv-codebar{display:flex;justify-content:flex-end;margin-bottom:5px}.rv-document .rv-codebar button{font:11px ui-monospace,monospace;color:var(--rv-faint);border:1px solid var(--rv-line);background:var(--rv-panel);border-radius:4px;padding:3px 7px;cursor:pointer}
.rv-document .rv-table{overflow:auto;margin:18px 0}.rv-document table{width:100%;border-collapse:collapse;font-size:13px}.rv-document th,.rv-document td{padding:10px 12px;border:1px solid var(--rv-line);text-align:left;min-width:110px}.rv-document th{font-weight:600;background:var(--rv-soft)}
.rv-document img{max-width:100%;height:auto}.rv-document hr{border:0;border-top:1px solid var(--rv-line);margin:24px 0}.rv-document .footnotes{font-size:12px;color:var(--rv-faint)}
`;

function textOf(node: ReactNode): string {
  return Children.toArray(node).map(child => typeof child === "string" || typeof child === "number" ? String(child) : isValidElement<{ children?: ReactNode }>(child) ? textOf(child.props.children) : "").join("");
}
function CodeBlock({ children }: { children?: ReactNode }) {
  const [status, setStatus] = useState("Copy code");
  return <div className="rv-code"><div className="rv-codebar"><button onClick={async () => {
    try { await navigator.clipboard.writeText(textOf(children).replace(/\n$/, "")); setStatus("Copied"); }
    catch { setStatus("Copy failed"); }
  }}>{status}</button></div><pre>{children}</pre></div>;
}

// Stable component types. Inline functions would remount links, images and
// tables on every render, dropping any text selection inside them.
const MARKDOWN_COMPONENTS: NonNullable<Parameters<typeof Markdown>[0]["components"]> = {
  a: ({ href, children }) => href ? <a href={href} target={href.startsWith("#") ? undefined : "_blank"} rel="noopener noreferrer">{children}</a> : <span>{children}</span>,
  img: ({ src, alt }) => src && /^https?:\/\//i.test(src) ? <img src={src} alt={alt ?? ""} loading="lazy" /> : <span>{alt || "Image unavailable"}</span>,
  pre: CodeBlock,
  table: ({ children }) => <div className="rv-table"><table>{children}</table></div>,
};
const REMARK_PLUGINS = [remarkGfm];
const safeUrl = (url: string) => /^(?:https?:\/\/|mailto:|#)/i.test(url) ? url : "";

// Raw HTML remains escaped. Relative source paths stay readable as text:
// they must not become links to arbitrary routes on the hosting application.
// Memoized: a message's Markdown is parsed again only when its text changes.
export const DocumentText = memo(function DocumentText({ text, definitions = "" }: { text: string; definitions?: string }) {
  const prefix = `note-${useId().replace(/[^a-zA-Z0-9-]/g, "")}-`;
  return <div className="rv-document"><Markdown remarkPlugins={REMARK_PLUGINS}
    remarkRehypeOptions={{ clobberPrefix: prefix }}
    urlTransform={safeUrl}
    components={MARKDOWN_COMPONENTS}
  >{definitions ? `${text}\n\n${definitions}` : text}</Markdown></div>;
});
