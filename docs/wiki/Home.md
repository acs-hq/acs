# ACS

ACS (Agent Control System) stops coding agents from overwriting each other when several of them work in one repository. ACS works with Claude Code today.

## The problem

Open two Claude Code sessions in one repository and neither knows the other exists. Both can edit the same file, and the last one to write wins. The other session's work is gone, and often nobody notices until much later. Add teammates with their own agents and it happens more often.

## What ACS does

ACS looks at each tool call an agent makes, before it runs, and allows or blocks it.

- **One writer per file.** The first session to edit a file holds it. Other sessions are told who holds it and can work on something else.
- **Protected paths.** Some paths, such as ACS's own settings and your CI workflows, are off limits to agents. People can still change them.
- **Dangerous commands.** Commands such as `git push --force` and deleting files the agent does not hold are blocked.
- **A check at commit time.** Git hooks catch what slipped past, such as an agent committing a file that another session holds.
- **Team sharing.** With a git remote, ACS shares who holds what with your teammates' machines on its own.

## What ACS does not do

- It does not merge code, resolve conflicts or review changes. Git and your code review still do that.
- It does not let agents talk to each other.
- It does not see people who do not run it. A teammate without ACS is invisible to it, and it does not block them.
- It is not a security boundary. It guards against accidents, not against someone trying to get around it. See [Limits](Troubleshooting-and-Limits#limits).

## Four words you will see

| Word | Meaning |
| --- | --- |
| **Claim** | One session's temporary hold on one file. While a session holds a claim, no other session can change that file. |
| **Sealed** | A path no agent may change. Only a person can. |
| **Pilot** | The person running an agent session. If you started the session, you are its pilot. ACS's messages to agents say things like "ask your pilot". |
| **Gate** | ACS's check on each tool call. "The gate is on" means ACS is enforcing its rules. |

## Pages

| Page | Read it if |
| --- | --- |
| [Install](Install) | You need the `acs` program. |
| [The initiator](The-Initiator) | You are the first person to adopt ACS in a repository. |
| [Teammates](Teammates) | Someone else has already adopted ACS in the repository. |
| [Configuring policy](Configuring-Policy) | You want to change the rules for your repository. |
| [Working with ACS](Working-with-ACS) | ACS blocked something, or you want to know how claims behave. |
| [Lore](Lore) | You want agents to know your conventions for part of the repository. |
| [Troubleshooting and limits](Troubleshooting-and-Limits) | Something looks wrong, or you want to know what ACS cannot do. |
