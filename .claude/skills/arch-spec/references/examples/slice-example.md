# Example: Canonical Slice Spec

> Source: `thyra/docs/world-design-v0/midnight-market-canonical-slice.md`
> Type: Canonical Slice — the system's minimum concrete instance

## What makes this good

### 1. Not an abstract template — a real instance
Has a name (Midnight Market), has concrete values, has real JSON.

### 2. Every component has concrete numbers
- 2 zones (Festival Square: 8 stalls, Creator Lane: 6 stalls)
- 2 entry gates (north: max 100/min, south: max 50/min)
- 3 chiefs (economy, safety, event)
- 5 change types
- 5 metrics

### 3. Initial state is complete, usable JSON
```json
{
  "worldId": "world_midnight_market_001",
  "zones": {
    "zone_a": { "name": "Festival Square", "stallCapacity": 8, "spotlightWeight": 0.6 }
  },
  "entryGates": {
    "north_gate": { "throttle": { "enabled": false, "maxPerMinute": 100 } }
  },
  "metrics": {
    "congestion_score": 0,
    "fairness_score": 1.0
  }
}
```
The dot-paths in this JSON align with the diff operations in change-proposal-schema.

### 4. Canonical story is a complete closure
safety_chief observes → north_gate congestion → proposes throttle_entry →
approved_with_constraints (60 min) → apply → congestion drops → precedent recorded

### 5. Cycle cadence has concrete numbers
- One cycle every 15 minutes
- Morning summary produced each night
- Each major change opens a 60-minute outcome window

---

> **Pattern notes:**
> - The core value of a canonical slice: grounding abstract concepts
> - Must have a concrete name, concrete values, concrete JSON
> - Initial state JSON dot-paths must align with the schema's diff operations
> - Must include a canonical story (complete closure walkthrough)
> - Must have concrete numbers for cycle cadence
> - This file is the litmus test for the entire spec stack: if you can't write it, the concept isn't concrete enough
