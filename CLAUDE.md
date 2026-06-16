# desolate-haven-cartography-engine

Native **Godot 4.6 + C#** terrain-authoring tool ("DHCE") driving a shared **Rust core**
(`dhce-core`) through a GDExtension. Authors the canon game world's base landscape (terrain + biomes
+ scatter) and exports an editable Godot scene. Part of the *Desolate Haven* project; the
`dhce-godot` adapter also feeds the `desolate-haven` game (`..\desolate-haven`).

## Build & test (Windows / PowerShell)

- Core tests: `cargo test -p dhce-core`. Build dir is kept outside the repo via `.cargo/config.toml`
  (`C:\Users\WSSco\.dhce-build`).
- DLL: build from inside `crates/dhce-godot` (`cargo build --release` — it's excluded from the
  workspace), then copy the `.dll` to `tool/addons/dhce/`.
- Full current-state + build/run/test hand-off: `docs/plans/dhce-tool-roadmap.md`.

## Story & lore canon (lives in the guide repo — this repo holds none)

- All world/story canon lives in the **guide repo** (`desolate-haven-guide`), the canonical home,
  rendered visually at `/lore` and `/story`:
  - world/lore (geography, magik, factions/races, locations, biomes) →
    `..\..\web\desolate-haven-guide\lore\`
  - narrative (characters, plot, dialogue) → `..\..\web\desolate-haven-guide\story\`
- **Biome canon ↔ this tool:** the 14-biome roster in `crates\dhce-core\src\biomes.rs`
  (`roster()` — labels + colors + landform + water) is the implementation of the canon biome
  palette. Keep labels and base colors consistent with canon
  `..\..\web\desolate-haven-guide\lore\biome-features.md` (the catalog — source of truth for
  per-biome palette/features) and `..\..\web\desolate-haven-guide\lore\biomes\` (per-region docs).
  Never invent biome lore here — raise it in the guide via the `/story-writing` skill.
- **Vault (shared AI reference):** distilled notes for the whole project live at
  `C:\dev\vault\projects\desolate-haven\`. Lore authoring dual-writes
  there from the guide repo; consume it read-only here.
- Rationale: guide `docs\adr\0001-lore-canonical-home.md` + game
  `..\desolate-haven\docs\adr\0006-lore-canon-moves-to-guide.md`.

## Where things are documented (three-repo project)

- **Guide repo** = canonical lore home + visual render (design system, `/lore`, `/story`) and the
  writing skills (`/story-writing`, `/character-development`, `/dialogue-writing`,
  `/plot-hole-validation`). Author lore there.
- **Vault** = the shared, AI-readable knowledge base (`vault\projects\desolate-haven\`).
- **This repo** + **the game repo** = engines that *consume* canon read-only via the cross-repo
  paths above.

## Documentation

- Working docs: `docs\adr\`, `docs\plans\`, `docs\specs\` with `status|date|owner` frontmatter.
