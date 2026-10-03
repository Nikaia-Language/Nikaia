# Why an automatic clone is not a convenience

**Date:** September 13, 2026
**Status:** an open question, written down so it is not mistaken for a small one.
It **decides nothing** — no rule here, no change to the emitter. Two records
already stand against an inserted clone, and this page says what they are and
what a proposal would have to do with them.
**Related:** [ADR-037](specification/adr/adr-037.md) D3 (`Shared` stays written by
hand, and the reason: sharing changes when a value is cleaned up) and D7 (the rule
that sorts written from inferred),
[ADR-005](specification/adr/adr-005.md) §3 (compiler-inserted clones, rejected:
"diagnostics suggest, the user decides"),
[ADR-283](specification/adr/adr-283.md) D3 (the same ban, argued again for the
view case), [ADR-297](specification/adr/adr-297.md) (cleanup, which is what the
reason is about), Part I [8.3](specification/10-nikaia-light.md) and Part II
[11.2](specification/20-nikaia-advance.md) (a task moves what it uses, so the
clone goes before the task is built)

`spawn` is `@detached`, so what the body uses moves into the task when the task
is built. Keeping a value in the parent therefore costs a hand-written
`.clone()`, placed *before* the `spawn` — and that is visibly a chore. The
obvious next proposal is that the compiler place it: see that the parent reads
the value afterwards, clone it quietly, and let both sides have one. It reads
like a comfort feature. It is not one.

A clone is one more owner, and one more owner means a later cleanup. That is not
a side effect of the convenience; it *is* the convenience. And "sharing changes
when a value is cleaned up" is the whole reason
[ADR-037](specification/adr/adr-037.md) D3 gives for `Shared` being written by
hand rather than guessed: inferring it "would move a destructor silently", and a
program can observe that it moved ([ADR-297](specification/adr/adr-297.md)). An
inserted clone produces the same observable change by a different route — the
compiler decides, without the program saying so, that a value now stops being
cleaned up where it used to be. D7 states the sorting rule the same way round:
*what is observable is written; what is not is inferred, with safe as the floor.*
A cleanup point is observable, so it is written.

So the question is not "may the compiler be helpful here". It is D3's question
over again, asked about a clone instead of about a `Shared` annotation, and the
prohibition has already been written twice on its own account:
[ADR-005](specification/adr/adr-005.md) §3 rejects implicit clone outright —
"Nikaia never inserts a clone the user did not write; diagnostics suggest, the
user decides" — and [ADR-283](specification/adr/adr-283.md) D3 re-argues it for
the view case rather than leaning on it. Whoever proposes an automatic clone owes
an answer to that reasoning: some account of why a moved cleanup point is
tolerable when inserted by this rule and not by the one D3 refused, and why the
`spawn` case is different from the two §3 already covers. Not an appeal to how
much typing it saves. Until such an answer exists, one clones by hand.
