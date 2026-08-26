# Developer Certificate of Origin

Pinvou Agent uses the [Developer Certificate of Origin 1.1](https://developercertificate.org/).

By adding a `Signed-off-by` trailer to a commit, you certify that you have the right to submit the contribution under the license of this repository and that the contribution meets the DCO 1.1 terms.

Create a signed-off commit with:

```bash
git commit -s
```

The trailer should match the commit author:

```text
Signed-off-by: Your Name <your.email@example.com>
```

Every commit in a pull request must be signed off. This is a developer attestation, not a GPG or SSH cryptographic signature.

How CI enforces this (`.github/workflows/dco.yml`): the check verifies that a `Signed-off-by:` trailer is present in each commit's message; it does not compare the trailer's name/email against the commit author — keeping them in sync is the developer's responsibility. Trusted Dependabot and GitHub Actions bot commits, and merge commits (more than one parent), are exempt.
