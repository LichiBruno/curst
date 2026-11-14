# Auth inheritance and workflows

## Auth types

In a request's **Auth** tab, **t** cycles through:

| Type | Sends |
|---|---|
| inherit | whatever the folder or collection above sets (the default) |
| none | nothing |
| basic | `Authorization: Basic …` from a user name and password |
| bearer token | `Authorization: Bearer <token>` |
| API key in a header | `<name>: <key>`, such as `X-Api-Key: …` |
| API key in the query | `?<name>=<key>`, on the first request only, never on a redirect |

Use variables rather than values: a token field holding `{{token}}`, with
`token` a secret variable.

## Inheritance

A request set to **inherit** uses the auth of the nearest folder above it that
sets one, and otherwise the collection's. Set it with **s** on the collection
or a folder, then **Auth**. The Auth tab of an inheriting request shows what it
gets, such as `bearer token from Shop`.

A request that sets its own auth keeps it. To move a whole collection onto one
auth:

- **A** on a request's Auth tab makes its auth the collection's, and has every
  request inherit it.
- **Make every request inherit**, in the collection's settings, does the same
  but keeps the collection's auth as it is.

Importing an OpenAPI spec puts the spec's own security on the collection, and
operations without a requirement of their own inherit it.

## Workflows

A workflow sends a few saved requests in order, keeping values from their
answers in secret variables. The usual one logs in and keeps a token:

1. Set the collection's auth to bearer token `{{token}}`.
2. Set the login request's auth to **none**, since it is what gets the token.
3. **w** in the sidebar, then **+ new workflow**. Name it, add a step with the
   login request, and under it keep a value: `token ← $.access_token`.
4. **Ctrl+S** saves it. Enter on it, or its name in the palette (**Ctrl+P**),
   runs it.

A value is taken from the answer's JSON body by a path such as
`$.access_token`, `data.items[0].id` or `$["odd key"]`, or from a header with
`header:X-Session`. It goes into a secret variable: the one already defined
nearest the request, otherwise in the active environment, otherwise in the
collection. A plain variable is never written to, since its value is in a
file.

A run shows each step as it goes. It stops at the first step that fails, an
answer that is not 2xx, or a value that is not there, and leaves every value
kept before in place. A step whose request needs approval to carry a secret
asks, then the run goes on. Steps follow their requests through renames and
moves.
