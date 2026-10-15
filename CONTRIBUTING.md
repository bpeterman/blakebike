# Contributing

Thanks for your interest in blakebike. Bug reports, hardware compatibility
notes, and patches are all welcome.

## Before you open a pull request

- Run the full check suite; CI runs the same commands:

  ```bash
  pnpm check
  pnpm test
  cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
  cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
  cargo test --manifest-path src-tauri/Cargo.toml
  ```

- Changes to device handling should say which hardware you tested against.
  Trainer and sensor behaviour varies enough between vendors that a change
  verified only in the simulator is hard to review.

## Contributor licensing

blakebike is released under the GNU General Public License v3.0 or later, and
the project is also offered under separate commercial terms. To keep that
possible, contributors grant the project maintainer the rights needed to
relicense their contributions.

By submitting a pull request, you agree that:

1. You are the author of the contribution, or you otherwise have the right to
   submit it under these terms.
2. You license your contribution to the maintainer under the GPL-3.0-or-later,
   **and** you additionally grant the maintainer a perpetual, worldwide,
   non-exclusive, royalty-free, irrevocable right to use, reproduce, modify,
   sublicense, and distribute your contribution under any other license terms,
   including proprietary ones.
3. You retain full copyright in your contribution and remain free to use it
   however you like elsewhere.

In short: the project stays GPL for everyone, and the maintainer keeps the
ability to relicense the combined work. If you are contributing on behalf of an
employer, please make sure you have their approval first.

Please add a `Signed-off-by` line to your commits (`git commit -s`) to record
your agreement to the above.
