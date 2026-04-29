# Code Maintenance Agent Security Model

- Repository reads use mock metadata in demo mode.
- Review comments require `PostReviewComment` approval.
- Patch proposals require `CreatePatchProposal` approval.
- Committed fixtures use patch/comment fingerprints, not raw proprietary code.
