## Overview

Prepares to release the version specified in the title.
Once all checks pass, the release should be good to go.

## Release steps

```shell
# Choose a version. It should be valid semver.
# Also, choose a branch name. A good default is `prep/$VERSION`.
VERSION=<VERSION>
BRANCH="prep/$VERSION"

# Make a branch for release prep and check it out.
git checkout -b $BRANCH

# Have cargo-release create the release.
# This does several things:
# - Validates that the git index is clean
# - Updates version numbers in the crates
# - Generates the changelog using `git-cliff`
# - Creates a commit with the changes
# - Pushes the branch to the remote
cargo release --no-publish --no-tag --allow-branch=$BRANCH $VERSION

# Open a PR; once tests pass and reviewers approve, merge to main and come back here for the final step.
# NOTE: We are here; this PR was created by this step.
gh pr create --base main --template .github/release_template.md --title "Prepare to release $VERSION"

# Finally, run `cargo release` on the main branch.
# This doesn't create new commits; it just tags the commit and pushes the tag.
git checkout main
git pull
cargo release
```
