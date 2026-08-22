# Software Bill of Materials

Pinvou Agent uses GitHub's dependency graph to maintain a live SPDX 2.3 Software
Bill of Materials (SBOM) for the public repository.

## Export the current source SBOM

From a GitHub CLI session with read access to the public repository:

```bash
gh api repos/Pinvou/pinvou-agent/dependency-graph/sbom \
  > pinvou-agent.spdx.json
```

The same dependency inventory is available from the repository's
[Dependency graph](https://github.com/Pinvou/pinvou-agent/network/dependencies).

The generated document covers dependencies detected from committed manifests
and lockfiles. Directly redistributed scripts, Skills, connectors, and assets
that need additional attribution are recorded in
[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) and in notices stored next
to their resources.

Release-specific packages may add platform files or privately built Official
components. Release-level SBOMs are handled by the manual release process:
`release-packages.yml` only produces the installers and their sha256 checksums
and does not create a GitHub Release. The manual release checklist must include
attaching or referencing an SBOM for the exact released artifacts, and an
Official package must not be described as reproducible from the Community
source tree.
