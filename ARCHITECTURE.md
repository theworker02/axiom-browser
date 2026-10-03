# Architecture

```text
URL
 │
 ▼
Networking  (axiom-net + axiom-url)
 │
 ├── HTTP/HTTPS
 ├── redirects
 └── caching hooks
 │
 ▼
HTML Parser (axiom-html)
 │
 ▼
DOM Tree (axiom-dom) ◄──── CSS Parser (axiom-css)
 │                              │
 └────────────┬─────────────────┘
              ▼
        Style Engine (axiom-style)
              │
              ▼
        Layout Engine (axiom-layout)
              │
              ▼
         Display List (axiom-paint)
              │
              ▼
           Raster / Present (axiom-paint + axiom-gfx)
              │
              ▼
            Window
```

`axiom-engine` owns orchestration and stage timings. `browser/desktop` is the host binary.

## Future differentiator

```text
DOM Mutation
     │
     ▼
Dependency Graph
     │
     ├── affected styles only
     ├── affected layout nodes only
     └── affected paint nodes only
               │
               ▼
        Incremental Renderer
```

Phase 1 remains sequential for correctness; introduce the graph and dirty tracking without rewriting the public pipeline facade.
