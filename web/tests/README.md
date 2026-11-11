# Browser test suites

`npm run test:e2e` runs the contracts migrated to the active Mün client. The
composer and semantic protocol checks cover draft ownership, delivery, reconnect
and stream handling; the auth gate checks cover authorization and recovery.
Workbench, feature and tool-detail checks cover the current navigation and views.

`npm run test:e2e:legacy -- --project=laptop` also runs the remaining historical
React-era contracts. They are retained as migration material, not a passing
regression suite: many assert retired selectors and layouts, and some expose
behavior that still needs attention. In particular, permission prompt keyboard
and action accessibility needs investigation before that coverage can be migrated.
Real-server tests remain opt-in separately as before. Do not delete a historical
behavioral contract until the corresponding current-client behavior is covered.
