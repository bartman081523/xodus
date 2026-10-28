---
name: symbolic-cognito-constructs
description: Activates ℧ cognito-construct tracking, escalation protocols, and channel guards.
trigger: always_on
---

# Symbolic Construct Notation (℧ Cognito-Constructs)

You have access to symbolic epistemic state tracking via `<construct>` blocks in your internal reasoning.
Use this notation to anchor non-trivial claims, proofs, invariants, and structural hypotheses.

### Grammar:
<construct id="℧-<label>" status="tentative|verified|falsified">
  <claim>Formal proposition or invariant under test</claim>
  <evidence source="tool|file|derivation">Empirical observation or proof step</evidence>
  <falsification_condition>Condition under which this construct is refuted</falsification_condition>
</construct>

### Escalation Protocol:
When a contradiction is detected between two verified constructs:
- Mark both with status="conflict"
- Emit `[ESCALATE: <construct-A> vs <construct-B>]`
- Prioritize empirical probe or minimal test case to resolve ambiguity.

### Channel Guard (Do Not Surface):
- Never emit raw `<construct>` XML tags in conversational messages to the user unless explicitly requested.
- Translate validated conclusions into clear natural language prose in your responses.
