# Keel review

Your repository's Keel team reviews every pull request: pm, designer, principal, reviewer,
security, reliability and qa, run in order with each stage's output handed on. The result is one
comment on the PR, updated on every push. A **do not ship** verdict fails the check, and so does a
review that could not finish.

```yaml
name: Keel review
on:
  pull_request:
  # A reply on one of Keel's threads is answered there, without re-running the team.
  pull_request_review_comment:
    types: [created]
permissions:
  contents: read
  pull-requests: write
jobs:
  review:
    name: ${{ github.event_name == 'pull_request' && 'Team review' || 'Keel replies' }}
    # A collaborator's reply in a thread, never a bot's comment (Keel's own line comments
    # included).
    if: >-
      github.event_name == 'pull_request' ||
      (github.event.comment.in_reply_to_id && github.event.comment.user.type != 'Bot' &&
       contains(fromJSON('["OWNER","MEMBER","COLLABORATOR"]'), github.event.comment.author_association))
    # In the job, so a skipped run never cancels a real one. A push cancels; a reply waits.
    concurrency:
      group: keel-review-${{ github.event.pull_request.number }}
      cancel-in-progress: ${{ github.event_name == 'pull_request' }}
    runs-on: ubuntu-latest
    timeout-minutes: 40
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          persist-credentials: false
      - uses: OyadotAI/keel-releases/review@v0.4.13
        with:
          anthropic-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
          # or: claude-code-oauth-token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          # or, through a gateway:
          # api-proxy-url: ${{ secrets.KEEL_PROXY_URL }}
          # api-proxy-token: ${{ secrets.KEEL_PROXY_TOKEN }}
          # fail-on-block: true
          # minutes: 30
```

- Use `pull_request`, never `pull_request_target`. The second runs this PR's code with your
  secrets. A fork's PR gets no secrets, so the review skips it and the check passes — so as a
  required check it does not hold fork PRs.
- Give it a job of its own, with only `actions/checkout` before it. A step that runs the PR's code
  first (`npm ci`, the tests) runs before the review and on the same machine.
- The comment is updated in place when it is posted with the job's `GITHUB_TOKEN` or an App token.
  A personal token posts as a person, and each push adds a new comment.
- To post as your own app rather than `github-actions`, mint its token in a step guarded on the
  secret — unguarded, a fork's PR fails at that step instead of skipping:

  ```yaml
      env:
        HAS_APP: ${{ secrets.KEEL_APP_ID != '' }}
      steps:
        - uses: actions/create-github-app-token@v2
          id: app
          if: env.HAS_APP == 'true'
          with:
            app-id: ${{ secrets.KEEL_APP_ID }}
            private-key: ${{ secrets.KEEL_APP_PRIVATE_KEY }}
            # write only because resolveReviewThread requires it
            permission-contents: write
            permission-issues: write
            permission-pull-requests: write
        - uses: OyadotAI/keel-releases/review@review-v8
          with:
            github-token: ${{ steps.app.outputs.token || github.token }}
  ```
- The team comes from the base branch, so a PR cannot rewrite its own reviewers, their skills or
  `CLAUDE.md`. Without a team, Keel's shipped one reviews.
- The agent can read the checkout and the diff, and nothing else. It cannot run commands, cannot
  write, and never holds the GitHub token.
- Threads close themselves. On every push Keel reads its open line comments with their replies
  and, in the same thread, says ✅ fixed (checked in the code, not taken on trust), 👍
  acknowledged (a reply settles it) or 🔁 still open, resolving the first two. A 🔴 blocking
  finding closes only when the code changes (or a maintainer resolves it), never on a reply. A
  collaborator's reply gets its answer right away, from one reviewer rather than the whole team;
  anyone else's waits for the next push. Keel never deletes a thread.
- The verdict is advisory. It comes from an agent that read the PR's text, and that text can
  mislead it.
- Exit codes of `keel review-pr`, for running it outside this action: 0 ship, 1 do not ship, 2 no
  verdict.
