
# v0.3.2

- General: Add color to CLI output.
- `extract`: Add `--squash-other` mode.
  - `--layers squash-other`: Squash all layers other than the base layer.

# v0.3.1

- `extract`: absolute symlinks are now correctly made relative to the target directory.

# v0.3.0

- Add `list` subcommand.
  - `--platform`: Select the platform to list.
  - `--username`: Authenticate to the registry using a username.
  - `--password`: Authenticate to the registry using a password.

# v0.2.0

- `extract`: Support filtering layers and files within them.
  - `--layer-glob`, `--lg`: Filter layer digests using a glob pattern.
  - `--file-glob`, `--fg`: Filter files within layers using a glob pattern.
  - `--layer-regex`, `--lr`: Filter layer digests using a regex pattern.
  - `--file-regex`, `--fr`: Filter files within layers using a regex pattern.

# v0.1.0

- Add `extract` subcommand.
  - `--platform`: Select the platform to extract.
  - `--username`: Authenticate to the registry using a username.
  - `--password`: Authenticate to the registry using a password.
  - `--layers`: Select which layers to extract.
  - `--overwrite`: Overwrite the existing target directory.
