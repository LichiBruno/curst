# Secrets and trust

## Where values are kept

The first time curst starts, it asks where to keep secret values:

- **The OS keychain**: the macOS Keychain, the Windows Credential Manager, or
  the Secret Service on Linux. curst checks it works before offering it; a
  Linux machine without a Secret Service (a server, an SSH session) should use
  the encrypted file.
- **An encrypted file**, `secrets.age`, locked with a passphrase that curst
  asks for once per session and keeps only in memory.

Switch at any time in Settings (**Ctrl+O**): every secret is copied, checked,
then removed from the old store.

Secret values never go into collection, environment or bundle files. Each
secret has a key such as `curst/<collection id>/env/<environment id>/token`,
where the collection id is made on your machine and never read from a shared
file, so a collection cannot reach another one's secrets.

## Hosts a secret may go to

A secret is sent only to the origins you approved for it: scheme, host and
port, so approving `https://api.example.com` approves neither plain `http` nor
another port. The first time a request would carry a secret somewhere new,
curst asks.

Redirects are checked too: a request carrying secrets follows a redirect only
within origins approved for all of them. A redirect elsewhere stops, and the
response shows it; **f** follows it through the same approval. Auth set on a
request is never sent across a redirect to another origin.

## Credentials typed in as plain text

curst flags values that look like credentials: `Authorization` and API-key
headers, `token` or `api_key` parameters, basic-auth passwords, and known
shapes such as JWTs and `sk_live_…` keys. A flagged value shows `⚠ …, saved as
plain text`; **m** turns it into a secret variable and leaves `{{name}}` in its
place. Imports of curl commands and specs do this on their own.

The check is a heuristic: a credential in an unusual place, such as inside a
JSON body, can get past it.

## Trust

A collection curst made is trusted. A `.curst/` folder found in a project is
not, until you say so (**T** in the sidebar): until then nothing in it is sent,
and no secret is used for it, so a repository cannot send your global
`{{github_token}}` somewhere of its choosing.

## From the command line

- `curst secrets list` shows every secret by name, with the hosts it may go
  to. Values are never shown.
- `curst secrets prune` deletes the secrets of collections this machine no
  longer knows.
- `curst run` with the encrypted file asks for the passphrase on the terminal,
  or reads `CURST_PASSPHRASE` when there is none, such as in CI. An
  environment variable is visible to other processes of the same user and can
  end up in CI logs: prefer your CI's secret storage, and never echo it.
