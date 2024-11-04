# Load Testing TON Connect Bridge

This directory contains load tests using [k6](https://k6.io) for testing the bridge server. This is to give the idea of how the service can be tested.

To get more info on the k6 tool itself, please follow the [official docs](https://grafana.com/docs/k6/latest/).

## Prerequisites

- Docker and Docker Swarm to run Redis and the app server
- k6 to run load tests

k6 doesn't natively support SSE, but there is an extension that implements it. To enable the extension, we need to build a k6 binary locally. Please follow the [extension's](https://github.com/phymbert/xk6-sse) readme to do it (it won't take long).

## Local Testing Scenario

The test is implemented in [local-clients.js](./local-clients.js) file.
The main goal of the local testing is to verify that the implemented bridge server can handle a very basic scenario with somewhat decent load. Obviously, the environment is far from the one that will be used in prod, so the local load test should be used more like a sanity check rather than a proper load testing mechanism.

Resources for the Redis instance and app instances can be found in the corresponding [docker-compose file](./docker-compose.loadtest.yml).

The test consists of two parts: clients that subscribe for events using the `/events` endpoint, and clients that send messages to random subscribed clients via `/message` endpoint.
There are 2000 subscribed clients, and 4000 messages in total are sent to randomly chosen clients every second. Every client is subscribed to 5 different "client ids", so overall there are 10k unique client ids.
From time to time, subscribed clients drop their current connection and establish a new one.

### Running the Tests

1. build the server image from the project root dir:

    ```bash
    docker build -t ton-bridge-rs:latest .
    ```

2. start service infra and the app using Docker Swarm and the provided compose file:

    ```bash
    cd load-tests
    docker stack deploy -c docker-compose.loadtest.yml ton-bridge-loadtest
    ```

    If needed, you can check application logs via:

    ```bash
    docker service logs --tail 100 -f ton-bridge-loadtest_server
    ```

3. run the load tests, assuming that you put the manually built the k6 binary to the `load-tests` folder

    ```bash
    ./k6 run local-clients.js
    ```

    This will start the tests and they will be running as long as configured in the scenarios.

### Test Results

These are results of running the tests on simple dev laptop inside Docker. Overall docker resources: 2 virtual CPU and 6GB Mem. Redis instance is the main component in the whole setup that consumes most of these resources.

Some non-important rows are omitted from the k6 output results to make it easier to read the results

```text
    scenarios: (100.00%) 2 scenarios, 4000 max VUs, 5m35s max duration (incl. graceful stop):
              * sseConnection: 2000 looping VUs for 5m10s (exec: sseConnectionScenario, gracefulStop: 10s)
              * sendMessages: 4000.00 iterations/s for 5m0s (maxVUs: 0-2000, exec: sendMessageScenario, startTime: 5s, gracefulStop: 30s)

    checks...........................: 100%  ✓ 4624462     ✗ 0

    http_req_duration................: avg=639.28ms min=211.45µs med=105.52ms max=5m4s     p(90)=568.97ms p(95)=699.13ms
    ✓ { name:open_sse_connection }...: avg=43.55s   min=211.45µs med=31.18s   max=5m4s     p(90)=1m39s    p(95)=2m6s
    ✓ { name:send_message }..........: avg=210.41ms min=593µs    med=102.77ms max=1.26s    p(90)=553.35ms p(95)=671.27ms
    http_req_failed..................: 0.00%   ✓ 0           ✗ 1151113

    sse_event........................: 1158168 3618/s
    sse_heartbeats_received..........: 6329    19/s
    sse_messages_received............: 1151839 3599/s
    sse_messages_sent................: 1151113 3596/s
```
