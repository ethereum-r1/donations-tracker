# donations tracker

track ETH donations to ethereum r1

(donation multisig: `eth:0xE73EaFBf9061f26Df4f09e08B53c459Df03E2b66`)

## TLDR

Simple backend microservice in rust that is responsible for: 

- tracking transfers of ETH to a certain address reliably (inlcuding internal transfers) 

- exposes an http server to fetch the historical list of transfers

## run

```
sudo docker-compose up --build
```
