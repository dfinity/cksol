- SHA-256 hash: `${CKSOL_MINTER_WASM_GZ_SHA256}`

## Deployments

| Type                | Canister ID                                                                                                  | Deployed? |
|---------------------|--------------------------------------------------------------------------------------------------------------|-----------|
| :rocket: Production | [`lh22c-kyaaa-aaaar-qb5nq-cai`](https://dashboard.internetcomputer.org/canister/lh22c-kyaaa-aaaar-qb5nq-cai) | :x:       |
| :test_tube: Staging | [`ljyxk-riaaa-aaaar-qb5mq-cai`](https://dashboard.internetcomputer.org/canister/ljyxk-riaaa-aaaar-qb5mq-cai) | :x:       |

## What's Changed

${CHANGELOG}

## Reproducible build

The attached `cksol_minter.wasm.gz` is built reproducibly. To verify the hash matches:

```bash
git checkout ${RELEASE_TAG}
just docker-build
sha256sum wasms/cksol_minter.wasm.gz
```
