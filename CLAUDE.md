# Working in this repository

The human does not read the code. They steer through three places, and every session keeps them
current.

| Question | Where | Rule |
| --- | --- | --- |
| What is open? | GitHub issues (`acs-hq/acs`) | Anything left open at the end of a session becomes an issue, or a comment on the one that covers it. |
| What waits for the human? | Issues labelled `decision-needed` or `human-task` | `decision-needed`: a choice only they can make; put the options in the issue. `human-task`: something only they can do (push, publish, accounts, sealed files, in-app checks). |
| What was decided, and by whom? | `docs/decisions/` and the index in its README | Every decision that is not obvious from the code gets a record that says `Decided by: the human` or `Decided by: Claude`, and a row in the index. |
| What is the plan? | `docs/PLAN.md` | A change of plan is a decision record plus a row in the plan's change table. |

- Write no notes, reports or prompts outside the repository. A prompt for a later session goes in
  the body of the issue for that work.
- Creating or editing issues and labels is public: show the human the list first, unless they
  asked for that issue.
- `docs/HANDOFF.md` is for the next session, not for the human. Do not put their to-do items only
  there.
- At the end of a session, tell the human which issues you opened or closed and which decisions
  you made without them.

Standing rules for implementation work (subagents on Sonnet, branches merged to `main` as several
commits, the human pushes, where `acs init` may run, Avast) are in `docs/HANDOFF.md`, section 2.
