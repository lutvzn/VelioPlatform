# References

Upstream code the server is checked against. The clones aren't committed;
check them out here at these versions:

| Folder        | Repository                              | Version                                    |
| ------------- | --------------------------------------- | ------------------------------------------ |
| `gotrue`      | https://github.com/supabase/auth        | v2.189.0                                   |
| `self-host`   | https://github.com/NuvioMedia/self-host | `30f156807ae77b2aec80b091b0d513841a25ab91` |
| `storage-api` | https://github.com/supabase/storage     | v1.60.4                                    |

```sh
git clone --branch v2.189.0 --depth 1 https://github.com/supabase/auth references/gotrue
git clone https://github.com/NuvioMedia/self-host references/self-host
git -C references/self-host checkout 30f156807ae77b2aec80b091b0d513841a25ab91
git clone --branch v1.60.4 --depth 1 https://github.com/supabase/storage references/storage-api
```

`apps/server/scripts/supabase-migrations.mjs` copies the schema from
`gotrue` and `self-host` into `apps/server/migrations`, and `--check` fails
when those copies drift. Without the clones, `--check` skips quietly.
