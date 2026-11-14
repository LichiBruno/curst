# Collections, files and environments

## Where things live

curst follows the XDG layout on macOS and Linux, and `%APPDATA%` on Windows.
`CURST_CONFIG_DIR` and `CURST_DATA_DIR` override both.

```
~/.config/curst/config.toml            settings, keys, theme
~/.config/curst/themes/<name>.toml     your themes
~/.local/share/curst/collections/<name>/
    collection.toml                    name, order, variables, auth, workflows
    environments/<env>.toml            an environment's id and variables
    <folder>/folder.toml               a folder's name, order and auth
    <folder>/<request>.toml            one request per file
~/.local/share/curst/globals.toml      global variables
~/.local/share/curst/registry.toml     the collections this machine knows
~/.local/share/curst/secrets-index.toml  secret names and approved hosts, no values
~/.local/share/curst/secrets.age       secret values, with the encrypted-file store
```

Everything is TOML, written atomically, so a crash never leaves half a file.
The files hold no secret values: a secret variable keeps only its name, and
its value lives in the secret store (see [Secrets and trust](secrets.md)).

## Requests, folders and order

A request's file name is a slug of its name (`List orders` →
`list-orders.toml`); its name lives inside the file. Each folder lists its
children in `order`, which the sidebar follows. A file you add by hand appears
at the end. Renaming or moving an item in curst renames or moves its file.

## A `.curst/` folder in a project

Run curst in a folder that holds a `.curst/` directory and that collection
opens too, so a team can commit its requests next to the code. Since anyone
can put files in a repository, a `.curst/` folder starts **untrusted**: you can
browse it, but nothing in it is sent and none of your secrets are used, until
you trust it (**T** in the sidebar). Trust belongs to the folder's path; a
folder moved outside curst is asked about again.

## Variables and environments

`{{name}}` works in the URL, parameters, headers, body and auth. A name is
looked up in this order:

1. the active environment of the request's collection (**Ctrl+E** to choose),
2. the collection's variables,
3. the global variables.

A variable can refer to others, as in `{{base_url}}/v1`. A request that uses a
variable no scope defines is not sent, and curst says which and where it is
defined, such as in an environment that is not in use.

Make a variable a **secret** with **s** in the variable editor, or **m** on a
value in a request that looks like a credential.

## Saving

**Ctrl+S** saves the open request. A new request asks for a name and where to
save it. Unsaved tabs show `●`, and closing one or quitting asks first.

A collection can save edits on its own: **s** on the collection, then **Save
edits without asking**. Edits are then saved a moment after you stop typing,
and before a tab closes. This choice is kept on your machine, never in the
collection's files.

## Bundles

A bundle is one file holding a whole collection: folders, requests,
environments, auth and workflows, without secret values. Export with **e** in
the sidebar, import with **i**, or use [`curst export` and `curst
import`](cli.md). An imported bundle becomes a new collection; the secrets it
names need their values set again.

Before writing a bundle, curst looks for values that seem to be credentials
typed in as plain text, and asks first.
