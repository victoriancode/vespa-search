# Vespa application package

This directory contains the Vespa application definition used by the vv-search deployment.
The `schemas/codesearch.sd` schema reflects the fields described in the architectural
specification and includes a semantic rank profile for ANN search. The embedding tensor
dimension is set to `x[768]`; update it to match the embedding model in use.

To build a zip for deployment from the repository root:

```sh
zip -r vespa-application.zip vespa/application
```
