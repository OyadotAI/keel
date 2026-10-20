# Keel review

Your repository's Keel team reviews every pull request: pm, designer, principal, reviewer,
security, reliability and qa, run in order with each stage's output handed on. The result is one
comment on the PR, updated on every push. A **do not ship** verdict fails the check, and so does a
review that could not finish.

```yaml
name: Keel review
on: pull_request
permissions:
  contents: read
  pull-requests: write
concurrency:
  group: keel-review-${{ github.event.pull_request.number }}
  cancel-in-progress: true
jobs:
  review:
    runs-on: ubuntu-latest
    timeout-minutes: 40
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
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
- The team comes from the base branch, so a PR cannot rewrite its own reviewers, their skills or
  `CLAUDE.md`. Without a team, Keel's shipped one reviews.
- The agent can read the checkout and the diff, and nothing else. It cannot run commands, cannot
  write, and never holds the GitHub token.
- The verdict is advisory. It comes from an agent that read the PR's text, and that text can
  mislead it.
- Exit codes of `keel review-pr`, for running it outside this action: 0 ship, 1 do not ship, 2 no
  verdict.
