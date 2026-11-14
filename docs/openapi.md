# Importing OpenAPI and Swagger specs

**o** in the sidebar, or `curst import`, takes any of:

- **a file**, JSON or YAML: OpenAPI 3.0 and 3.1, and Swagger 2.0;
- **a folder**: every spec in it and the folders under it, skipping what
  `.gitignore` leaves out; with several, you choose one;
- **a URL**: the spec itself, or a documentation page. For Swagger UI and
  Redoc pages, curst reads the page (it never runs it) to find the spec's
  address, follows initializer scripts, reads a spec written into the page,
  tries the page's `-json` sibling as NestJS serves it, then the usual paths
  such as `/openapi.json` and `/v3/api-docs`.

Docs behind a login? Give the URL `{{variables}}`, or paste a curl command that
fetches it; its credentials become secrets, and are sent only to that origin.

## What it becomes

- one folder per tag, one request per operation, named after its summary;
- the servers become environments with a `base_url`, the first in use;
- path parameters become variables; required query parameters and headers are
  on, optional ones are added off;
- the body comes from the spec's example, or a skeleton made from the schema;
- security schemes become auth that points at secret variables. The spec's
  own requirement goes on the collection, and operations inherit it.

A preview shows the counts and anything worth knowing before anything is
written. Examples that look like credentials are left out, with a placeholder
in their place.

## Refreshing

A collection made from a spec remembers where it came from. **u** fetches it
again and shows what changed:

- endpoints that are new,
- endpoints the spec now describes differently, each as a diff,
- endpoints that are gone.

You choose what to take. Requests you edited are only replaced if you choose
them, and one with unsaved edits in a tab is held back. **U** changes where the
spec is; if the file or folder moved, curst asks for the new place.
