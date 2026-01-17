# Vespa application package

This directory contains the Vespa application definition used by the vv-search deployment.
The `schemas/codesearch.sd` schema reflects the fields described in the architectural
specification and includes a semantic rank profile for ANN search. The embedding tensor
dimension is set to `x[768]`; update it to match the embedding model in use.

To build a zip for deployment from the repository root (so `services.xml` is at
the root of the zip), run:

```sh
(cd vespa/application && zip -r ../../vespa-application.zip .)
```

Alternatively, use the helper script:

```sh
./scripts/build_vespa_application_zip.sh
```

⚠️ Do **not** run `zip -r vespa-application.zip vespa/application` because that
creates a zip with `services.xml` nested under `vespa/application/`, which Vespa
rejects.
