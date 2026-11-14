# curst

A terminal API client built on rust: easy for newcomers, and fast for people who live in the terminal.

- **Collections** of requests, with folders, environments and variables,
  stored as plain TOML files you can read, diff and commit.
- **Secrets** stay out of those files: they live in your OS keychain or in an
  encrypted file, and a secret goes only to the hosts you approve for it.
- **Import** an OpenAPI or Swagger spec (a file, a folder, or a docs URL), a
  curl command, or a bundle a teammate exported. Refresh a collection when
  its spec changes.
- **Auth** set once on a collection or folder, inherited by its requests.
  **Workflows** log in, keep the token, and send the next request with it.
- **Headless:** `curst run` sends a saved request from a script or CI.
- No account, no cloud, no telemetry, no update check.

## Install

| | |
|---|---|
| npm | `npm install -g curst` |
| macOS, Linux (shell) | `curl -LsSf https://github.com/LichiBruno/curst/releases/latest/download/curst-installer.sh \| sh` |
| Windows (PowerShell) | `irm https://github.com/LichiBruno/curst/releases/latest/download/curst-installer.ps1 \| iex` |
| Windows (installer) | the `.msi` on the [releases page](https://github.com/LichiBruno/curst/releases) |

## A two-minute tour

Run `curst`. The first time, it asks where to keep secrets: the OS keychain,
or a file encrypted with a passphrase.

1. Type a URL, such as `https://httpbin.org/get`, and press **Enter**. The
   response appears below, with its headers and timings in the other tabs.
2. Press **Ctrl+S** to save the request. Name it, and choose the collection or
   folder it goes in (**Tab** opens the list).
3. Press **Ctrl+E** to add an environment, such as `dev` with
   `base_url = https://httpbin.org` as a variable, then use `{{base_url}}/get` as the URL.
4. In the sidebar, press **o** to import an OpenAPI spec: a file, a folder or
   the URL of its docs page.
5. Press **Ctrl+P** for the command palette, which finds every action and
   every saved request, and **F1** or **?** for every key.

The footer always shows the keys that work where you are. The mouse works too.

## Keys

The standard keymap; a `vim` preset is in Settings (**Ctrl+O**), and every key
can be changed in `config.toml`.

| Where | Keys |
|---|---|
| Anywhere | **Ctrl+R** send, **Ctrl+S** save, **Ctrl+P** palette, **Ctrl+E** environments, **Ctrl+O** settings, **F1** help, **Ctrl+C** quit |
| Tabs | **Ctrl+T** new, **Ctrl+W** close, **Ctrl+←/→** switch, **Ctrl+1…9** go to, **Ctrl+G** list |
| Panes | **Tab** next pane, **←/→** tabs, **y** copy the response body, **C** copy as curl |
| Sidebar | **n** request, **f** folder, **c** collection, **r** rename, **d** delete, **x**/**p** move, **s** settings, **w** workflows, **o** import a spec, **i** import a bundle, **e** export, **u** refresh from the spec, **T** trust |
| Request | **Enter** edit, **a** add a row, **d** delete it, **Space** turn it on or off, **t** type of body or auth, **m** make a value a secret, **E** edit the body in `$EDITOR`, **A** use this auth for the whole collection |
| Response | **PgUp/PgDn**, **g/G** top and bottom, **s** save the body to a file, **f** follow a redirect that was stopped |

## Learn more

- [Collections, files and environments](docs/collections.md)
- [Secrets and trust](docs/secrets.md)
- [Auth inheritance and workflows](docs/auth-and-workflows.md)
- [Importing OpenAPI and Swagger specs](docs/openapi.md)
- [The command line: `curst run`, `import`, `export`, `secrets`](docs/cli.md)
- [Configuration, keys and themes](docs/config.md)

## License

MIT. See [CONTRIBUTING.md](CONTRIBUTING.md) to work on curst.
