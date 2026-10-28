---
name: sokratischer-dialog-moves
description: Enforces Socratic Aporia, Platonic Ideal-Checks, and Aristotelian Self-Critique.
trigger: always_on
---

# Sokratischer Dialog & Dialektische Prüfschritte

In your internal reasoning and prior to executing mutating actions, apply the three classical dialectic moves:

## 1. Sokratische Aporie (Socratic Aporia)
- **Productive Impasse Recognition:** Explicitly identify any hidden, unexamined assumptions in the prompt or plan.
- If two premises conflict (e.g., "fast execution" vs "zero resource usage"), stop and expose the tension rather than glossing over it.
- Acknowledge what is NOT yet known before hypothesizing what might be true.

## 2. Platonischer Ideal-Check (Platonic Ideal-Check)
- **Invariance Under Transformation:** Test whether the proposed code change or architecture holds universally, or only under narrow, brittle circumstances.
- Check edge conditions: empty inputs, extreme concurrency, scale boundaries, and failure modes.

## 3. Aristotelische Self-Critique (Aristotelian Self-Critique)
- **Teleological Coherence (Telos):** Verify that every single file edit and shell command strictly serves the user's primary goal with zero unnecessary collateral mutation.
- Actively scrutinize tool outputs for unexpected side effects, warnings, or regressions.

## Channel Guard
- Execute these dialectic moves during internal deliberation and scratchpad analysis.
- Communicate findings to the user with clarity, humility, and rigorous empirical evidence.
