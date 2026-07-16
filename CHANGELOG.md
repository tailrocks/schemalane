# Changelog

All notable changes to Schemalane are documented here.

## [Unreleased]

### Changed

- Advisory-lock defaults are now derived from `(schema, history table)` instead of the
  legacy fixed key `7333654209921337`. This removes guaranteed global contention
  between different migration targets in one database; because the target suffix is a
  32-bit CRC, distinct targets can still collide. During a mixed-version rollout, old
  and new runners targeting the same schema do **not** exclude each other because they
  acquire different keys. Keep runners single-version during rollout, or pass
  `--advisory-lock-id 7333654209921337` to new runners until every runner is upgraded.
