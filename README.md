# template-rust

Template repository for a Rust project.

TODOs for a new project:
- [ ] Change the license if MPL2 is not appropriate for the project. Make sure to do this before adding any code.
- [ ] Ensure the dev docs (in particular the release and compatibility semantics) are valid for this project.
- [ ] Set [CODEOWNERS] to the team that owns the repository.
- [ ] Create an API user in [FOSSA] and store it as a secret named `FOSSA_API_KEY`.
  - Consider naming it with the pattern `ci-{REPO_NAME}`. For example, `ci-template-rust`.
- [ ] Update repository permissions as appropriate. Generally, the CODEOWNER team is set as admin.
- [ ] Update branch protection rules as appropriate.
- [ ] Update repository features and settings. Recommended defaults:
  - [ ] Turn off all features (Wikis, Issues, Sponsorships, Discussions, Projects); FOSSA uses other systems for these.
  - [ ] Only allow squash merging.
  - [ ] Always suggest updating PR branches.
  - [ ] Allow auto-merge.
  - [ ] Automatically delete head branches.

Then just edit the included Rust project, or remove it and `cargo init` your project, and get going!

[codeowners]: https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners
[fossa]: https://app.fossa.com

# recommendations

- If publishing a Linux binary, consider providing two: one that [statically links libc](./docs/dev/reference/static-binary.md), and one that doesn't.
- If publishing a macOS binary, consider providing two: one for [Intel and one for M-series CPUs](./docs/dev/reference/macos-arch.md).
- If this application may be used on AWS Graviton or similar, consider providing an ARM build for Linux as well.
