# Changes to the vendored engine

`vendor/` is skatecore-cpp's `clean/` (see `vendor/REVISION`), copied in by
`scripts/cleanroom/sync-lite.sh`, which replaces the whole folder. These changes were made on top of
it and need carrying into skatecore-cpp before the next sync from there, or they are lost. They are
kept as commits in a repository started from this vendored copy (github.com/rhythmerc/skatecore,
same layout: `sync-lite.sh <its path>` brings them back):

- **`sk8_rider_set_walk_view`** (`include/sk8.h`, `src/api.cpp`, `src/rider.hpp`, `src/rider.cpp`
  `Rider::walk`). On foot the left stick turned the walker (x, 2.6 rad/s) and walked them on or back
  (y): tank controls, and in AR relative to nothing the player can see. Given a view (the direction
  the player looks along) the stick is theirs: the walker goes the way it is pushed at once and turns
  to face it (14 rad/s). Without a view the old behaviour is unchanged. Tests:
  `on_foot_with_a_view_...` and `on_foot_without_a_view_...` in `src/lib.rs`.
