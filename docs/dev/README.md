
# development

Tags denote releases.
Any commit merged to `main` is expected to be release ready,
with the exception of the `version` in `Cargo.toml`.
For more detail, see the [release process](#release-process).

Follows [semver](https://semver.org/):
- MAJOR version indicates a user facing breaking change.
- MINOR version indicates backwards compatible functionality improvement.
- PATCH version indicates backwards compatible bug fixes.

The initial beta releases use `0` as the major version; when this changes to `1`
it will not necessarily indicate a breaking change, but future major version increases will.

## compatibility

- Tracks the latest version of the Rust compiler and associated tooling at all times.
- Tracks the latest Rust language edition.
- Aggressively upgrades dependencies. Relies on testing to validate dependencies work.

## setting up your development environment

Recommend Visual Studio Code with the `rust-analyzer` extension.
Install Rust here: https://www.rust-lang.org/tools/install

These tools may be useful, although they're not required:
```
cargo nextest # https://nexte.st/
cargo upgrade # https://lib.rs/crates/cargo-upgrades
cargo machete # https://lib.rs/crates/cargo-machete
```

### cross compilation

Sometimes, especially when debugging a platform build issue, it's useful to "cross compile" the project
from your home operating system to a destination operating system.

Steps to do so are located in the [cross compilation reference](./reference/cross-compile.md).

## style guide

Make your code look like the code around it. Consistency is the name of the game.

You should submit changes to this doc if you think you can improve it,
or if a case should be covered by this doc, but currently is not.

Use `rustfmt` for formatting.
CI enforces that all changes pass a `rustfmt` run with no differences.
CI ensures that all patches pass `clippy` checks.

Comments should describe the "why", type signatures should describe the "what", and the code should describe the "how".

We use the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/about.html)
during code review; if you want to get ahead of the curve check it out!

Ideally, every PR should check for updated dependencies and update them if applicable;
if this is not realistic at minimum every non-bugfix release **must** ensure dependencies are up to date.

The `.cursor/rules/conventions` file also describes more specific code conventions for Cursor and other
compatible agents; but keep in mind _these are optional for humans_ even though they are _recommended_.

## release process

> [!NOTE]
> Make sure to merge commits to `main` with meaningful commit messages.
> Using [conventional commits](https://www.conventionalcommits.org) format
> is recommended for better organization of changes in the GitHub release notes.

To create a new release, simply create and push a tag:

```shell
# Create and push the tag from main branch
git checkout main
git pull && git pull --tags --force
git tag v0.5.0 # replace this with the version you want to release
git push --tags
```

Once you push the tag, the GitHub Actions workflow will:
1. Set the crate versions based on the tag
2. Build the release binaries for all supported platforms
3. Generate release notes from commit messages
4. Create a GitHub release with the binaries and notes

This approach ensures the version in Cargo.toml always matches the release tag, and eliminates the need for separate version update PRs.
