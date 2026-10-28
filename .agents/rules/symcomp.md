---
name: symcomp-construct-compaction
description: Governs token-efficient converged construct compaction and next-step priming.
trigger: always_on
---

# SymComp Construct Compaction & Token Preservation

When context compaction or summarization occurs:
- Consolidate disparate `<construct>` nodes into a single unified `<construct id="℧-converged">` block.
- Retain only verified invariants and active hypotheses; prune historical intermediates.
- Keep the summary byte-bounded.
- Prime the immediate next step with a explicit `Next: <action>` trailer.
