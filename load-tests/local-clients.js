import { SharedArray } from 'k6/data';
import { check } from 'k6';
import http from 'k6/http';
import exec from 'k6/execution';
import sse from "k6/x/sse"
import { randomItem, randomString } from 'https://jslib.k6.io/k6-utils/1.2.0/index.js';
import encoding from 'k6/encoding';
import { Counter, Trend } from 'k6/metrics';


const connectionsCount = 2000;
const clientIDsPerConnection = 5;

export const options = {
    systemTags: ['scenario', 'status', 'name', 'error_code'],
    // to show http_req_duration metric separately for /events and /message endpoints
    // in the output results
    thresholds: {
        'http_req_duration{name:open_sse_connection}': ['avg>0'],
        'http_req_duration{name:send_message}': ['avg>0'],
    },
    scenarios: {
        sseConnection: {
            exec: 'sseConnectionScenario',
            executor: 'constant-vus',
            vus: connectionsCount,
            duration: '5m10s',
            gracefulStop: '10s',
        },
        sendMessages: {
            exec: 'sendMessageScenario',
            executor: 'constant-arrival-rate',
            startTime: '5s',
            duration: '5m',
            preAllocatedVUs: 0,
            maxVUs: 2000,
            rate: 4000,
            timeUnit: '1s',
        },
    },
};

const messagesSentCounter = new Counter('sse_messages_sent');
const messagesReceivedCounter = new Counter('sse_messages_received');
const heartbeatsReceivedCounter = new Counter('sse_heartbeats_received');

const baseUrl = "http://127.0.0.1:3000"

const sharedClientIDs = new SharedArray('wallet client ids', function () {
    return Array.from(
        { length: connectionsCount * clientIDsPerConnection },
        (_, i) => `id_${i}_${randomString(50)}`,
    );
});

export function sseConnectionScenario() {
    // VU id is one-based
    const connectionNum = exec.vu.idInTest - 1;
    const indexFrom = connectionNum * clientIDsPerConnection;
    const indexTo = indexFrom + clientIDsPerConnection;
    const clientIDs = sharedClientIDs.slice(indexFrom, indexTo);

    const url = `${baseUrl}/events?client_id=${clientIDs.join(',')}`;

    const params = {
        method: 'GET',
        tags: { name: 'open_sse_connection' },
    };
    const response = sse.open(url, params, function (client) {
        client.on('open', function open() {
            // console.log(`connected ${connectionNum} with ids ${clientIDs}`);
        });

        client.on('event', function (event) {
            check(event, { "expected event type": (e) => e && ['heartbeat', 'message'].includes(e.name) });
            if (event.name == 'message') {
                messagesReceivedCounter.add(1);
                check(event, {
                    "non empty id": (e) => e.id.length > 5,
                    "correct data": (e) => {
                        const data = JSON.parse(e.data);
                        const from = data['from'];
                        const message = encoding.b64decode(data['message'], 'std', 's');
                        return from.length > 5 && message.startsWith("Hello from Client:");
                    }
                })
            } else if (event.name == 'heartbeat') {
                heartbeatsReceivedCounter.add(1);
            }
            // randomly drop connections from time to time
            if (Math.random() < 0.01) {
                client.close();
            }
        });

        client.on('error', function (e) {
            console.log('an unexpected error occurred: ', e.error());
        });
    })

    check(response, { "sse response status is 200": (r) => r && r.status === 200 });
}

let message = encoding.b64encode("Hello from Client: " + randomString(250));

export function sendMessageScenario() {
    const fromClient = randomItem(sharedClientIDs)
    const toClient = randomItem(sharedClientIDs);

    const url = `${baseUrl}/message?client_id=${fromClient}&to=${toClient}`;

    const response = http.post(url, message, {
        headers: { 'Content-Type': 'text/plain' },
        tags: { name: 'send_message', }
    });
    const pass = check(response, { 'message sent to client': (r) => r.status === 200 });
    if (pass) {
        messagesSentCounter.add(1);
    }
}
