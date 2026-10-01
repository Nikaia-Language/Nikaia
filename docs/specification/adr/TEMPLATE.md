# ADR-NNN: <The decision, written as a statement>

*Copy this file to `adr-NNN.md`, fill every section, delete these italic notes.
The title is the answer, not the topic: "A list has no `+`", not "Operators on
lists".*

**Status:** Proposed | Accepted | Superseded by [ADR-MMM](adr-MMM.md)
**Date:** <day the decision was taken>
**Answers:** <the question, with the issue or `open-decisions.md` entry that
asked it>
**Supersedes / Related:** <records this changes or leans on, one line each: what
it says that matters here>

*There is no `Built:` line and no target version. Whether and when a decision
is built is the issue's and the CHANGELOG's to say; a record is not edited as
the code catches up (see the [index](README.md)).*

## 1. Question

*One question, in a sentence a reader can answer yes/no or pick from a list.
Then: what is blocked without an answer, and why this question has to be
decided here rather than defaulting. If it is really two questions, write two
records.*

## 2. How others do it

*What other languages and tools do about this same question, so the decision
is made knowing the field and not in a vacuum. At least two, from different
families where they exist (a systems language, a managed one, a functional
one, the tool people already use). For each:*

| Language / tool | Its answer | What it costs there | Source |
| :--- | :--- | :--- | :--- |
| | | | link, version, or "from memory, not checked" |

*Say what does **not** transfer and why (a garbage collector, a different
ownership model, an ecosystem we do not have). Mark anything not verified; do
not present recollection as fact.*

## 3. Options, weighed

*Every option the field or the team suggests, including doing nothing. For each:
what it is, one concrete example of the source a developer writes, then how it
fares against the four weights of [ADR-258](adr-258.md). Fill the table with
answers, not adjectives: who pays, what is written, what it costs at run time,
how it reads next to a neighbouring rule.*

### Option A — <name>

```nika
// the code a developer writes under this option
```

### Option B — <name>

```nika
```

### Weighed

| | D1 the special case pays, not the common one | D2 the compiler works, not the developer | D3 no cost for the common case | D4 consistent with itself | If it is wrong |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **A** | | | | | what it costs to undo, and who notices |
| **B** | | | | | |

*Where a number decided it (an instruction count, a size, a latency), give the
number and link the measurement; the method lives in the notes, not here.
Where two weights pull apart, say so and say which one the language's promise
to the program (D1, D3) or to the reader (D2, D4) is being traded for.*

## 4. Decision

*The chosen option, stated as rules a reader can apply, each numbered (D1, D2,
…) so that later records and code can cite one. Say which weight decided where
the table did not. Say in a sentence why each rejected option lost.*

### D1 — <rule>

## 5. Consequences

*What changes for a reader of the specification, for a program, for the
compiler: stated as what is now true or now costs more, not as a to-do list.
Link the issue(s) that carry the work; do not report their progress. Name the
cost this decision accepts.*

## 6. What this does not decide

*Questions the decision leaves open on purpose, each with where it will be asked
(an issue or an `open-decisions.md` entry). Leave the section out if there are
none.*

---

## What does not belong in a record

* **How far it is built**, which version built it, or a list of what was found
  while building it: that is the issue and the CHANGELOG.
* **How the decision was reached**: sessions, rounds, who asked what, what was
  tried first. Only the reasoning that settles the question stays.
* **The state of another list** ("the backlog is empty", "the status page says
  …"). A record is true on the day it is read, not only on the day it was
  written.
* **Citations of a backlog or a status note as an authority.** A rule that
  matters is stated in the record in its own words.
* **A second specification.** The rule a program follows belongs in Part I–III;
  the record says why it is that rule and links to it.

*Before marking a record Accepted: can a reader who has never seen this project
answer "why is it this way, and what else could it have been"? If a sentence
only makes sense to someone who watched the work happen, cut it.*
