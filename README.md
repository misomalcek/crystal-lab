# crystal-lab

Point at a folder. The app builds a 3D crystal from the files, embeds the
text locally, and retrieves. Qdrant and the nomic embed model travel inside
the `.app`. Nothing else to install.

Provided as is. MIT license. No telemetry, no accounts, nothing leaves your
machine unless you set an OpenAI-compatible endpoint yourself.

## Minimum requirements

Measured on this build machine, not assumed:

| | | how to repeat |
|---|---|---|
| macOS | 27.0 (Apple Silicon) | `sw_vers` |
| CPU | arm64 only — Intel and Windows are not in this dmg | `uname -m` |
| Disk after install | 188 MB | `du -sh src-tauri/target/release/bundle/macos/crystal-lab.app` |
| RAM while indexing | not measured on a large folder this run | Activity Monitor while Ingest runs |
| Extra software | none (no Docker, Postgres, Python, Node) | — |

This app is not notarized by Apple. On first launch macOS will say the
developer cannot be verified. Right-click the app → Open → Open.
You only need to do this once.

## What it does

1. Pick a folder. The graph is files, import/link/sibling edges, and after
   ingest the text chunks hanging off each file.
2. Ingest embeds chunks with **nomic-embed-text-v1.5** (768d) into a local
   Qdrant process. Ports are picked free at launch — never 6333 or 8083.
3. Names that survive a deterministic filter (no model required) become one
   node per normalized spelling. The same name in two files is one node, with
   `mentions` edges into those files and `relates_to` edges where two names
   share a chunk. A spelling fold is kept as a merge note. Embedding-near
   names with different slugs are flagged `conflicts_with` and stay two nodes.
4. Click a node. The card shows type, files, neighbors by weight, the passages
   the name was seen in, betweenness next to degree, and the raw payload.
   After a retrieve, a passage that was a hit also shows its raw score and its
   fused score.
5. Two sliders sit on the crystal. **Hub spread (LinLog)** runs from 1 to 15.
   It pushes highly connected nodes apart, and it widens the ring of chunks
   around each file. 1 is the floor, because 0 collapses the core. **Glow**
   runs from 0 to 1.5 and scales edge haze. Each folder remembers its own
   pair. The starting pair is 4 and 0.45, not a claim that every corpus
   wants those numbers.
6. Hop and Blast walk the edges from the focused node (one step, or two).
   Ask can call the same two walks, plus retrieve, when the endpoint accepts
   tool calls. An endpoint that refuses tools still answers from the excerpts.
7. Retrieve fuses three ranked lists with Reciprocal Rank Fusion (`k=60`,
   Cormack et al. 2009): vector, lexical (BM25 over the folder), and graph
   expansion along `relates_to`. Fusion reads **rank**, never cosine, never
   edge weight.
8. Optional OpenAI-compatible endpoint (cloud or local, same fields): assigns
   a type, may withhold a name from the verse without deleting it, and unlocks
   ask-your-sources. It does not merge two different slugs. Saving or removing
   an endpoint rebuilds the verse. A bad endpoint is an error and you stay in
   default mode.

Graph expansion is the Zep / Graphiti pattern: the graph produces a third
ranked list, then RRF. This build does **RRF only** — not Zep's extra MMR
or node-distance stages.

## Limitations

This is a free app for a folder of notes or a small repository. It is not for
a massive source tree or a large codebase. The stop is in the app, not in
Qdrant. Past the caps below, ingest finishes and the log names what was cut.

```
400 chunks for the whole folder          extract.rs  MAX_CHUNKS
80 chunks for one file                   extract.rs  MAX_CHUNKS_PER_FILE
1 800 characters per chunk, 200 overlap  extract.rs  CHUNK_CHARS, CHUNK_OVERLAP
2 MB of text in one file                 extract.rs  MAX_TEXT_BYTES
32 MB for one PDF                        extract.rs  MAX_PDF_BYTES
400 file nodes drawn                     graph.rs    MAX_GRAPH_NODES
10 000 files walked                      census.rs   MAX_FILES
```

A chunk is about 1 800 characters of the file. Four hundred of them is the
whole embedding. A small project fits. A large one stops at the first 400
and the log says `truncated at 400 chunks`.

Ingested as text, then embedded:

```
md txt html htm
rs ts tsx js jsx mjs cjs py go java c h cpp cc hpp
pdf   (text layer only)
```

Drawn, not embedded:

```
png jpg jpeg          graph node, no OCR
doc docx ppt pptx     counted, not parsed
```

Skipped entirely: `node_modules`, `target`, `dist`, `.git`, any hidden
directory, and names ending in `_files`. Import edges are drawn for
`import`, `from`, `require(`, and a Rust `mod` declaration when the target
file is in the folder.

## Build

```bash
npm install
CI=true npx tauri build --bundles app
hdiutil create -volname crystal-lab \
  -srcfolder src-tauri/target/release/bundle/macos/crystal-lab.app \
  -ov -format UDZO src-tauri/target/release/bundle/dmg/crystal-lab_0.4.2_aarch64.dmg
```

`tauri build --bundles dmg` currently cleans the `.app` and has packed a thin image
that omitted Qdrant and nomic. Build the app bundle, then wrap it with `hdiutil`.

Apple Silicon. Qdrant lives at `src-tauri/resources/embed-runtime/qdrant` (and a
copy at `src-tauri/binaries/qdrant-aarch64-apple-darwin` for the host triple).
Adding Intel is another binary in that folder, not a rewrite.

## Size of this dmg

```
ls -l src-tauri/target/release/bundle/dmg/crystal-lab_0.4.2_aarch64.dmg
# 134257741 bytes · 134.3 MB SI / 128.0 MiB   (2026-10-05)

du -sh src-tauri/target/release/bundle/macos/crystal-lab.app
# 188M
```