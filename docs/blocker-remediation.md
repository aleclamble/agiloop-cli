# Blocker Remediation

When a task is blocked by CI, review feedback, missing credentials, merge
conflicts, or another external blocker, Scheduler should treat the fix as a
separate remediation work packet.

## Expected Flow

1. Detect the blocker on the original item.
   Examples: failed CI check, review-requested changes, merge conflict, provider
   summary with `blocked_reason`, or an operator action such as "ask agent to
   fix CI".
2. Create a linked remediation work packet.
   The packet references the original job, run, branch, PR, check URL, and
   failure evidence. It is not a retry of the original task.
3. Execute the remediation in an isolated worktree on a new branch.
   The branch should be scoped to the blocker, for example
   `scheduler/remediate-ci/<original-slug>/<run-id>`.
4. Commit and push the remediation only when the job delivery mode allows code
   delivery.
5. Notify the user with the remediation branch or PR, the original blocked item,
   validation performed, and explicit merge instructions.
6. After the user merges the remediation, re-check the original blocked item.
   If the original CI/review/blocker is now green, mark the original item
   unblocked. If not, create another linked remediation packet with the new
   evidence.

## Non-Goals

- Do not mutate the original task branch in place.
- Do not create a duplicate of the original feature/report task.
- Do not auto-merge remediation work unless a job explicitly permits automated
  merging.
- Do not hardcode repository names, CI workflow names, or provider-specific
  behavior into the remediation flow.

## Provider Prompt Contract

Provider agents must receive enough context to patch the blocker without
guessing:

- original repository path and remote
- original branch or PR URL
- failed check name and check URL
- relevant log excerpt
- expected validation command when known
- required output: remediation branch/PR, commits, validation, and merge note

If a provider cannot access the external system or lacks credentials, it should
write a clear blocked summary instead of modifying the original task branch.
