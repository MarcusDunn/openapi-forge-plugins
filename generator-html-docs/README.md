# generator-html-docs

OpenAPI Forge generator that emits a static HTML documentation site from
an OpenAPI spec. Distributed as an OCI WASM component at
`ghcr.io/marcusdunn/generator-html-docs:<tag>`.

## What it produces

A multi-page static site, one page per endpoint:

```
out/
  index.html                           # landing: info + tag tree
  _static/styles.css
  _static/app.js
  tags/<tag>/index.html                # nested per OAS 3.2 tag.parent
  operations/<operationId>.html        # one canonical page per endpoint
  schemas/<schemaId>.html              # per-schema reference pages
```

The site uses semantic HTML (`<nav>`, `<aside>`, `<main>`, `<article>`,
`<section>`, `<dl>`, `<details>`), renders Markdown in every prose slot
the spec defines (descriptions, summaries, examples), and works with no
JavaScript — the bundled script only handles the theme toggle and
remembering collapsed sidebar nodes.

## Differentiators vs Swagger UI

- **Nested tags** (OAS 3.2 `tag.parent`) rendered as a real tree.
- **One page per endpoint** — deep-linkable, printable, browser-back
  friendly.
- **Always-expanded** descriptions and schemas — no accordion hiding
  content from a reader.
- **Markdown everywhere**, including parameter descriptions.
- **Screen-reader-clean** semantic markup; works in reader mode.

## Configuration

```toml
[generator]
oci = "ghcr.io/marcusdunn/generator-html-docs:v0.1.0"
config = { theme = "dark", includeSchemas = true }
```

See `schema.json` for the full config surface.

## License

Dual-licensed under Apache-2.0 or MIT, at your option.
