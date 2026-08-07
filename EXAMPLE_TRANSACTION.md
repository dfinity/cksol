Here is an example of a [SOL deposit transaction](https://solscan.io/tx/4ufenqv8AWdSDeU3q9N8239n19oQRvBrP1J9uGv9VnHaddcEswQpmXLjCbcCmQLYi1vcD4E2zD7aUsA5366XHdYn) from a wallet to a deposit address, showing the information that is returned.

```shell
curl --location 'https://api.mainnet.solana.com' \
--header 'Content-Type: application/json' \
--data '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getTransaction",
    "params": [
       "4ufenqv8AWdSDeU3q9N8239n19oQRvBrP1J9uGv9VnHaddcEswQpmXLjCbcCmQLYi1vcD4E2zD7aUsA5366XHdYn",
        {
            "encoding":"jsonParsed",
            "maxSupportedTransactionVersion":0
        }
    ]
}'
```

```json
{
  "jsonrpc": "2.0",
  "result": {
    "blockTime": 1770381594,
    "meta": {
      "computeUnitsConsumed": 450,
      "costUnits": 1784,
      "err": null,
      "fee": 80000,
      "innerInstructions": [],
      "logMessages": [
        "Program ComputeBudget111111111111111111111111111111 invoke [1]",
        "Program ComputeBudget111111111111111111111111111111 success",
        "Program ComputeBudget111111111111111111111111111111 invoke [1]",
        "Program ComputeBudget111111111111111111111111111111 success",
        "Program 11111111111111111111111111111111 invoke [1]",
        "Program 11111111111111111111111111111111 success"
      ],
      "postBalances": [
        17595721,
        10000000,
        1,
        1
      ],
      "postTokenBalances": [],
      "preBalances": [
        27675721,
        0,
        1,
        1
      ],
      "preTokenBalances": [],
      "rewards": [],
      "status": {
        "Ok": null
      }
    },
    "slot": 398443979,
    "transaction": {
      "message": {
        "accountKeys": [
          {
            "pubkey": "6TqNg48mSd5evmY66JVfGeGTwszrU1YLCeSw3GJ2qsUC",
            "signer": true,
            "source": "transaction",
            "writable": true
          },
          {
            "pubkey": "387njEppTfLSwaGN2hgSNUTMpbCZgu2xaiEY9Nj1QPbG",
            "signer": false,
            "source": "transaction",
            "writable": true
          },
          {
            "pubkey": "11111111111111111111111111111111",
            "signer": false,
            "source": "transaction",
            "writable": false
          },
          {
            "pubkey": "ComputeBudget111111111111111111111111111111",
            "signer": false,
            "source": "transaction",
            "writable": false
          }
        ],
        "instructions": [
          {
            "accounts": [],
            "data": "3b1H8Rq1T3d1",
            "programId": "ComputeBudget111111111111111111111111111111",
            "stackHeight": 1
          },
          {
            "accounts": [],
            "data": "LKoyXd",
            "programId": "ComputeBudget111111111111111111111111111111",
            "stackHeight": 1
          },
          {
            "parsed": {
              "info": {
                "destination": "387njEppTfLSwaGN2hgSNUTMpbCZgu2xaiEY9Nj1QPbG",
                "lamports": 10000000,
                "source": "6TqNg48mSd5evmY66JVfGeGTwszrU1YLCeSw3GJ2qsUC"
              },
              "type": "transfer"
            },
            "program": "system",
            "programId": "11111111111111111111111111111111",
            "stackHeight": 1
          }
        ],
        "recentBlockhash": "8QFcHAXJZZXSTbHqbZHUtZerSG4SEWauP6q4Xa5nGitW"
      },
      "signatures": [
        "4ufenqv8AWdSDeU3q9N8239n19oQRvBrP1J9uGv9VnHaddcEswQpmXLjCbcCmQLYi1vcD4E2zD7aUsA5366XHdYn"
      ]
    },
    "version": "legacy"
  },
  "id": 1
}
```
