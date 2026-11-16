# Third-party notices

Velio Platform is licensed under the GNU Affero General Public License,
version 3 (`LICENSE`). Some files come from, or are adapted from, the
projects below, which keep their own licences.

| Files                                     | From                                                                                                        | Licence                                                |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| `apps/server/migrations/0001_gotrue.sql`  | [Supabase Auth](https://github.com/supabase/auth) v2.189.0, generated from its migrations                   | MIT, `licenses/supabase-auth-MIT.txt`                  |
| `apps/server/migrations/0002_storage.sql` | [Supabase Storage](https://github.com/supabase/storage) v1.60.4, the part of its schema Nuvio uses, adapted | Apache-2.0, `licenses/supabase-storage-Apache-2.0.txt` |
| `apps/server/migrations/nuvio/*.sql`      | [Nuvio Self-Hosted](https://github.com/NuvioMedia/self-host) at `30f1568`, unchanged                        | Apache-2.0, `licenses/nuvio-self-host-Apache-2.0.txt`  |
| `apps/server/smoke/*.sh`                  | Nuvio Self-Hosted's smoke tests, ported                                                                     | Apache-2.0, `licenses/nuvio-self-host-Apache-2.0.txt`  |

`references/README.md` lists the exact upstream versions, and
`apps/server/scripts/supabase-migrations.mjs` regenerates the copied
migrations from them.

## NOTICE from Nuvio Self-Hosted

```
Nuvio Self-Hosted
Copyright 2026 Nuvio Media

This distribution includes modified configuration derived from the Supabase
self-hosted Docker distribution.
Copyright 2024 Supabase
Licensed under the Apache License, Version 2.0.

The secret-generation approach includes portions derived from work by Inder
Singh, also licensed under the Apache License, Version 2.0:
https://github.com/singh-inder/supabase-automated-self-host

Individual container images and runtime dependencies retain their respective
copyright and license terms.
```
