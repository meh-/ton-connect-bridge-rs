# TON Connect HTTP Bridge

## What is this?

TLDR: It is an implementation of [HTTP Bridge API](https://github.com/ton-blockchain/ton-connect/blob/main/bridge.md) from [TON Connect specification](https://github.com/ton-blockchain/ton-connect).

Bridge is a transport mechanism to deliver messages from the app to the wallet and vice versa.

* **Bridge is maintained by the wallet provider**. App developers do not have to choose or build a bridge.
* **Messages are end-to-end encrypted.** Bridge does not see the contents or long-term identifiers of the app or wallet.
* **Communication is symmetrical.** Bridge does not distinguish between apps and wallets: both are simply clients.
* Bridge keeps separate queues of messages for each recipient’s **Client ID**.

## Why?

As of Nov 2024, there isn't an open source implementation of the bridge. There are only two, so called, reference implementations, that cannot be used in production: [bridge](https://github.com/ton-connect/bridge) and [bridge2](https://github.com/ton-connect/bridge2). So many wallets developers either have to implement the bridge spec by themselves, or to use the only popular bridge provider. None of them open-sourced their solutions, which doesn't sound good for The Open Network.

The main idea of this implementation is to encourage wallet developers to host the bridge by themselves.
