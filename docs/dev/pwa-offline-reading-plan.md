# Offline reading: separate implementation plan

Not part of PWA hardening. The current offline page makes no download or sync
promise. Implement in this order when this feature is scheduled:

1. Define account/session ownership and policy for logout, revoked access,
   session expiry, incognito, and shared devices. No cross-account fallback.
2. Build a public offline-capable reader/library shell with explicit local
   account unlock/ownership checks, independent of authenticated SSR/RSC.
3. Add Download for offline to issue actions. Persist ordered metadata and
   page assets with integrity/completeness state, resume/cancel/retry controls,
   and an offline library. Never advertise incomplete downloads as readable.
4. Add quota estimates, per-issue usage/removal, persistent-storage requests
   after user intent, and graceful denial/eviction handling. Separate transient
   thumbnail eviction from user-requested downloads.
5. Introduce an account-scoped durable outbox for progress/bookmarks. Define
   idempotency, latest-write ordering, concurrent-device conflict policy,
   schema migration, and reconnect/authentication handling. Show pending/error
   sync state without claiming background synchronization is guaranteed.
6. Test airplane-mode cold launch, partial downloads, quota exhaustion,
   permission denial, expired sessions, account switching, app termination,
   corrupted assets, and interrupted updates on real mobile devices.

Keep separate from this plan:

- Push: opt-in UX, VAPID management, subscription persistence/expiry, relevant
  job hooks, notification deep links, and badge clearing/privacy policy.
- File/share intake: supported formats, authenticated pending intake, size/type
  limits, parser validation, duplicate handling, and explicit review before import.
