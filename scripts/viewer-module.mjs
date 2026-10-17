// Bundles modules of packages/viewer with esbuild, so the tests and the protocol
// browser check run the TypeScript the viewer ships. A module, not a script: it
// is outside test/ and named so that `node --test` does not take it for a test.
import { createRequire } from "node:module";
import { build } from "esbuild";

const viewer = new URL("../packages/viewer/", import.meta.url).pathname;

// One bundle that re-exports every export of each named module, such as
// "encryption" for packages/viewer/encryption.ts. `options` go to esbuild.
export async function bundleViewer(modules, options = {}) {
  const contents = modules.map((name) => `export * from "./${name}";`).join("\n");
  const { outputFiles } = await build({ stdin: { contents, resolveDir: viewer, loader: "ts" }, bundle: true, write: false, format: "esm", ...options });
  return outputFiles[0].text;
}

// The same bundle, imported. A CommonJS bundle is run with a require that
// loads its `external` packages, such as React, from the workspace; an import
// from a data: URL could not.
export async function importViewer(modules, options = {}) {
  const text = await bundleViewer(modules, options);
  if (options.format !== "cjs") return import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
  const module = { exports: {} };
  new Function("require", "module", "exports", text)(createRequire(import.meta.url), module, module.exports);
  return module.exports;
}
