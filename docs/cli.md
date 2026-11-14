# The command line

`curst` with no command opens the terminal UI. The commands below run without
it, for scripts and CI. They use the same collections, variables and secrets.

## `curst run`

```sh
curst run 'Shop/orders/List orders'
curst run shop/orders/list-orders --env staging --json
```

Sends a saved request, named by its collection, folders and name as the
sidebar shows them; slugs work too, case does not matter, and a name holding a
`/` of its own, such as `GET /pets`, is fine.

| Option | |
|---|---|
| `-e, --env NAME` | the environment to use; the one chosen in curst otherwise |
| `--allow-host HOST` | let the request's secrets go to this host for this run, though it was never approved for them |
| `--json` | print status, headers, timings and body as one JSON object |
| `--timeout SECONDS`, `-k/--insecure`, `--cacert FILE` | as for the UI |

The body goes to stdout, so it pipes, and the status line to stderr:

```sh
curst run Shop/orders/list | jq '.items[0]'
# stderr: HTTP/2 200 · 142 ms
```

The request is sent as in the UI: its variables resolved, its auth inherited,
its secrets read from the store. With the encrypted-file store, curst asks for
the passphrase on the terminal, or reads `CURST_PASSPHRASE` when there is no
terminal. A secret that would go to a host not approved for it stops the run,
unless that host is given with `--allow-host`.

Exit codes:

| Code | |
|---|---|
| 0 | answered 1xx, 2xx or 3xx |
| 1 | answered 4xx or 5xx |
| 2 | a usage or setup problem: no such request, an undefined variable, a secret not approved, a locked store |
| 3 | not sent: the connection failed, timed out, or was refused |

## `curst import`

```sh
curst import ./openapi.yaml
curst import ./services --spec api/v2/openapi.json
curst import https://api.example.com/docs
curst import shop.curst.toml
```

Imports a spec (see [OpenAPI](openapi.md)) or a bundle as a new collection,
with its first environment in use, and says what came in. In a folder with
several specs, `--spec` picks one.

## `curst export`

```sh
curst export Shop shop.curst.toml
```

Writes a collection as a bundle, without secret values. It refuses when the
collection holds what looks like a credential in plain text; `--force` writes
it anyway.

## `curst secrets`

```sh
curst secrets list     # every secret by name, with the hosts it may go to
curst secrets prune    # delete the secrets of collections that are gone
```

Values are never printed.
