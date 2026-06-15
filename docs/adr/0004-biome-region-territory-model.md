---
status: active
date: 2026-06-15
owner: wsscottca
---

# ADR 0004 — Trait-composition world model (traits → biome → region → territory)

## Status
Accepted. (Rewritten 2026-06-15 from the first same-day draft — see §History. The first draft's
flat "biome = fixed archetype" model is superseded by the trait-composition model below; its
Region/Territory scopes survive.)

## Context

N3's biome work started flat: 14 `BiomeDef`s each with one `color`, one `biome_r` id per cell.
A first slice shipped a 7-slot palette + elevation ramp on that model. Authoring against it
exposed a deeper need: the author wants to **compose** a place from independent factors — "this
area is forest, this is rocky, this is marshy," with jaggedness, height, foothills, vegetation,
moisture each dialed separately — and have borders **transition naturally** ("a jagged-rock,
evergreen-foothill mountain easing into rolling forested hills, then plains"). A single biome id
can't express that, and naming a *place* "Jagged Mountains" wrongly bakes a **trait** (jagged)
into a **proper noun**.

The resolution: make the **traits** the primitives, let **"biome" be the culmination** of the
traits at a place (a descriptor, not the source data), and keep the **named canon places as
Regions**. This is a weight/trait-field + classifier pattern, and it renders cleanly through
Godot vertex colors / a terrain splat shader (extends engine systems; ADR 0002 principle).

## Decision

### 1. Four concepts

| Concept | What it is | Role |
|---|---|---|
| **cell** | the mesh primitive (today's core "region"; rename deferred) | smallest unit fields live on |
| **Trait fields** | independent per-cell layers (§2) — the *adjectives* | the source of truth; paintable, blendable |
| **Biome** | `classify(trait fields)` → a descriptor (e.g. *jagged evergreen highland*) | the **culmination**; derived + lockable |
| **Region** | a named canon place (the 14: Sacred Woods, Great Lake…) — the *proper nouns* | identity (name, `--mk-*` accent); **may span several biomes** |
| **Territory** | a painted area that overrides the fields under it | the on-top authoring/override scope |

Composition order, each building on the last: **Traits compose → a Biome describes the mix → a
Region is a named area of it → a Territory overrides.** Place names stay canon — the 14 names are
defined in the guide repo (`biome-features.md` / `geography.md`); the tool references them. Trait
adjectives (jagged, rolling, forested) are *not* place names. (If a place should be renamed so a
trait word is freed — e.g. *Jagged Mountains → Dwarven Mountains* — that edit happens in the guide
via `/story-writing`, then flows here.)

### 2. Trait fields (the 9 primitives)

Each is a resident per-cell layer in `World`, like `elevation` is today — paintable and blended
independently across borders (so each transitions at its own rate):

| Trait | Type | Drives |
|---|---|---|
| **elevation** | scalar | base height (existing) |
| **jaggedness** | scalar | peak sharpness |
| **relief** | scalar | foothills / hilliness amplitude |
| **foothill_falloff** | scalar | width of the mountain→plain skirt |
| **erosion** | scalar | crisp ↔ eroded (scree, smoothing) |
| **temperature** | scalar | snow/frost vs arid — *decouples snow from altitude* (cold lowland Frozen Reaches; warm high peaks stay green) |
| **moisture** | scalar | marsh, rainfall, river flow (existing) |
| **vegetation** | enum | barren / grass / scrub / forest / evergreen / marsh / thorn — surface tint now, scatter later |
| **palette_family** | enum (7) | the shared base ramp (§4) |

Scalar traits blend as continuous fields; the two enums flip per cell but their **rendered
colour** blends via the existing neighbour-smoothing + the transition buffer, so borders read soft.
(Ordered "vegetation laddering" through the buffer — evergreen→forest→grass — is a future
enhancement, not v1.)

### 3. Biome = classifier (the culmination)

`classify(trait fields) → biome label` over coarse buckets of vegetation × landform (elevation +
jaggedness + relief) × climate (temperature + moisture). It feeds **display / export /
scatter-by-biome** — **never the render path**, which reads the trait fields directly, so the
classifier is off the critical path and can stay simple. **Lockable:** painting a biome preset
(a named trait bundle — Forest, Plains, Rocky, Marsh…) can pin an explicit label on those cells.

### 4. Seven shared base palettes

A small, hue-disciplined set of light→dark terrain ramps — the world's colour substrate:
**Verdant · Arid · Stone · Ashen · Frost · Wetland · Exotic.** The `palette_family` trait picks
one; **vegetation** tints the cover band and **temperature** blends the high band toward snow/frost.
Variety comes from the 9 trait dials layered over few bases → strong cohesion. A Region's identity
= base + vegetation + `--mk-*` accent + landform. (Supersedes the 14 distinct palettes from the
first slice.)

### 5. Tools

- **Brush** — paints a single trait *or* stamps a biome-preset bundle into a footprint, and flags
  a **transition buffer** band where it meets cells with different values.
- **Border tool** — outlines an area at a chosen **scope: Biome / Region / Territory**.
- **Select** — flood-select contiguous similar cells (existing `select_contiguous`), feeds the editor.

### 6. Engine mechanics

- Trait fields are resident `Vec`s in `World`. A brush edit sets target values in its footprint
  and flags the buffer.
- A deterministic **blend pass** (param-field diffusion: Laplacian averaging over mesh adjacency)
  smooths each scalar field across the buffer; one global **Transition width** → diffusion `k`.
  Idempotent layered model (base fields + recorded edits), mirroring the elevation base/shaped/
  terrain layering.
- **Render:** `ground_color = base_palette[palette_family].ramp(elevation, temperature)` tinted by
  `vegetation`, read from the smoothed fields; `--mk-*` accent reserved for borders / markers /
  scatter tint.
- **Determinism:** all of the above is lerp / averaging / threshold + the existing deterministic
  `fbm2` — no new transcendentals (ADR 0001/0002 contract intact).

### 7. Bottom-up staging

Design for all of it; build in order, each visible on its own:

1. **Trait substrate** — add the 9 fields to `World`; 7 base palettes; render resolves
   `palette × vegetation × temperature × elevation`. *(reworks the first palette slice; Rust + DLL)*
2. **Biome classifier** — `classify(traits) → label`, lockable. *(Rust + DLL)*
3. **Brush + transition buffer + blend pass** — paint traits, auto skirts, Transition width. *(Rust + C#)*
4. **Border tool (scopes) + Region tier** — the 14 as named Regions; outline/select; accent identity. *(Rust + C#)*
5. **Per-trait editor panel + biome/region presets.** *(C#)*

## Consequences

- The first palette slice (`docs/plans/n3-biome-palette.md`) is **partly superseded**: the ramp +
  per-cell colour path + smoothing stay; the 14 palettes collapse to 7 bases; the 14 `BiomeDef`s
  become **Region presets**; `biome_r` (one id/cell) is replaced by the trait fields + the
  classified label.
- New core state: 7 scalar trait `Vec`s + 2 enum `Vec`s, a blend pass, a classifier, base-palette
  + vegetation tables. Larger than the flat model but each field is independent and testable.
- The "region → cell" rename stays deferred but committed; new APIs use cell / Trait / Biome /
  Region / Territory.
- `docs/specs/n3-tooling-design.md` §8 records the same; the staged plan is
  `docs/plans/n3-biome-traits.md`.

## History
- 2026-06-15 (first draft): "Biome = fixed archetype (7-slot palette + landform + water), one id
  per cell; Region/Territory tiers." Shipped one slice on it. Superseded same day by the
  trait-composition model above after the author reframed biomes as a *composition* of independent,
  blendable traits with biome as the emergent culmination.
