# Pursuit and current-use evidence gate

Planning starts by deciding whether there should be a build, not by assuming the nouns in a request still describe the product. Pass this gate before naming a destination, preserving a surface, choosing an API version, or drafting a contract. Reapply it whenever later evidence changes what appears to be live.

## Establish the live outcome

1. State the user or operational goal without inheriting the requested implementation.
2. Inventory every named product surface, entity, workflow, API, or command that the proposed change would preserve, version, or rebuild.
3. For each name, classify it as `current`, `legacy_or_internal`, or `mixed`, and name the live product concept it represents. A symbol, schema field, endpoint, test, generated contract, or deployed callable existing in the repository is not evidence that the product concept is currently used. Legacy wire names may remain necessary while their product vocabulary is obsolete.
4. Establish current use with concrete evidence: a live caller or entry point, runtime observation, recent usage data, a current product artifact, or explicit human confirmation. Record the source and what it demonstrates. Search for absence and replacement paths as deliberately as for confirming references.

If current-use evidence is absent, stale, contradictory, or only proves that code exists, fail closed. The first planning question must establish whether the live concept is still used and should survive. Do not spend another round comparing implementations, compatibility strategies, versions, or migration scope first.

## Test whether to pursue

Compare all of these against the stated goal:

- `pursue_now`: the smallest justified change and why it is worth doing now;
- `defer`: what is lost by waiting and a concrete revisit condition, if any;
- `cancel`: whether the goal is obsolete, already satisfied, or unsupported by evidence;
- `no_build`: the cheapest operational, documentation, configuration, or behavior-preserving route;
- `delete_or_retire`: whether removing the unused surface or stopping its preservation is cleaner than rebuilding it.

Choose the fastest, simplest, cleanest goal outcome. Existing code, a detailed request, prior planning effort, and compatibility anxiety are not reasons by themselves to pursue. When deletion or retirement is plausible but consequential, ask the human before designing the replacement. A guided mapper may record an accepted defer or cancel outcome; an autonomous planner may produce a contract only for a fully evidenced `pursue_now` outcome.

## Autonomous evidence payload

An autonomous `planned` result must include `pursuit_evidence` beside `contract_markdown`:

```json
{
  "state": "planned",
  "pursuit_evidence": {
    "goal": "Implementation-independent outcome",
    "current_use_evidence": [
      {"source": "Concrete file, command, runtime observation, usage record, or human statement", "observation": "What this proves is currently used"}
    ],
    "named_concepts": [
      {"name": "Name from the request or repository", "status": "current|legacy_or_internal|mixed", "live_concept": "Current product concept", "basis": "Concrete classification basis"}
    ],
    "decision_test": {
      "pursue_now": "Why the smallest build is justified now",
      "defer": "Why waiting loses to pursue now",
      "cancel": "Why cancellation loses to pursue now",
      "no_build": "Why the cheapest no-build route is insufficient",
      "delete_or_retire": "Why removal or retirement is not the cleaner outcome"
    }
  },
  "contract_markdown": "<complete contract.md>"
}
```

Every string must be substantive, `current_use_evidence` and `named_concepts` must be non-empty, and the evidence must establish current use of the live concept rather than mere code existence. Missing or uncertain evidence requires `needs_user`. The independent reviewer verifies the evidence against the repository and task; it never treats the planner's payload as proof by assertion.
