# The Nikaia Specification

What the language **is**. Normative: if the compiler disagrees with a rule
here, one of the two is a bug.

| | covers |
| :--- | :--- |
| [Part I — The Language Core](10-nikaia-light.md) | the language a person learns first: values, ownership, errors, the two build switches |
| [Part II — Concurrency and Metaprogramming](20-nikaia-advance.md) | concurrency, parallelism, build-time reflection (`comptime`), the grammar protocol |
| [Part III — Tooling](30-nikaia-tooling.md) | the CLI, the manifest, the contract ledger, `std`, the diagnostics contract |

The three parts are written to one **language standard**,
[`STYLE.md`](STYLE.md): a section states the rule, then its consequence, and
nothing else. **Why** the language is this way lives next door, one record per
decision: [the ADRs](adr/README.md); what is built lives in the
[roadmap](../project_status_and_roadmap.md) and the
[changelog](../../CHANGELOG.md). The specification does not point at them.

`scripts/check-spec-blocks.py` guards an editorial pass: every ```` ```nika ````
block of the three parts is a test, and the script reports a pass that changed
one.

For what belongs in this directory and what belongs in the notes, see
[`docs/README.md`](../README.md).
