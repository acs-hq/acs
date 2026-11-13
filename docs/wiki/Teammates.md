# Teammates

This page is for you if someone else has already adopted ACS in your repository. That person did the work described in [The initiator](The-Initiator). You only do the steps below, once per clone.

## Your steps

1. Install ACS (see [Install](Install)).
2. Pull the main branch, so that you have the commit that adopted ACS. Do this before the next step.

   ```bash
   git pull
   ```

3. Run `acs init` once in your own clone.

   ```bash
   acs init
   ```

4. Accept the ACS plugin when Claude Code offers it.
5. Start a new Claude Code session.
6. Check the result.

   ```bash
   acs doctor
   ```

   Look for `0 failure(s)` and no warning on the `Claude Code plugin` line.

## Why you need `acs init`

`acs init` keeps the committed policy. Its output says `kept` for each policy file, and you do not choose any settings.

You still need it because two things are local to each clone and do not arrive with `git pull`:

- the git setting that fetches other people's claims (a claim is one session's temporary hold on one file)
- any git hooks that the repository does not track

## Do not skip the install

Without the `acs` program on your `PATH`, the plugin's hooks do nothing and say nothing. Your agents would run with no checks and you would see no error, and other people's claims would not protect their files from your agents. The git hooks only print a one-line warning and let the commit through.

## What you get and what you do

| What | How you get it |
| --- | --- |
| Policy files | `git pull` |
| Plugin setting | `git pull`, then accept the plugin in Claude Code |
| Hooks tracked by the repository | `git pull` |
| Hooks not tracked by the repository | `acs init` |
| The git setting that fetches claims | `acs init` |
| The `acs` program | You install it |
| Other people's live claims | Automatic after `acs init`, if the repository switched team sync on (`acs status` shows it on the `Remote:` line) and you can reach the remote |

With team sync on, after `acs init`, `git branch -r` lists entries such as `origin/acs/claims/<id>`. These are not branches anyone works on: they are how ACS shares claims. See [How claims reach your team](The-Initiator#how-claims-reach-your-team).

Next: [Working with ACS](Working-with-ACS).
