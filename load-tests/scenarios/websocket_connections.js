import ws from 'k6/ws';
import { check, sleep } from 'k6';
import { Counter, Trend } from 'k6/metrics';
import { register } from '../helpers/auth.js';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const WS_URL = BASE_URL.replace('http://', 'ws://').replace('https://', 'wss://');

const wsConnections = new Counter('ws_connections_established');
const wsMessageLatency = new Trend('ws_message_latency');

export const options = {
    scenarios: {
        websocket_steady: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '15s', target: 500 },
                { duration: '30s', target: 1000 },
                { duration: '15s', target: 1000 },
                { duration: '10s', target: 0 },
            ],
        },
    },
    thresholds: {
        ws_connections_established: ['count>900'],
        ws_message_latency: ['p(99)<100'],
    },
};

export function setup() {
    // Create a batch of users for WebSocket testing
    const users = [];
    for (let i = 0; i < 10; i++) {
        const user = register(
            `wstest_${Date.now()}_${i}`,
            `ws_${Date.now()}_${i}@test.com`,
            'testpassword123'
        );
        if (user) users.push(user);
    }

    if (users.length === 0) {
        throw new Error('No users could be registered for WebSocket test');
    }

    return { users };
}

export default function (data) {
    // Round-robin across pre-created users
    const user = data.users[__VU % data.users.length];
    const token = user.access_token;

    const url = `${WS_URL}/gateway`;

    const res = ws.connect(url, {}, function (socket) {
        socket.on('open', function () {
            wsConnections.add(1);

            // Send identify payload
            socket.send(JSON.stringify({
                op: 1,
                d: { token: token },
            }));
        });

        socket.on('message', function (msg) {
            const data = JSON.parse(msg);

            // Respond to heartbeat requests
            if (data.op === 10) {
                const start = Date.now();
                socket.send(JSON.stringify({ op: 2 }));
                wsMessageLatency.add(Date.now() - start);
            }
        });

        socket.on('error', function (e) {
            console.error('WebSocket error:', e.error());
        });

        // Keep connection alive for the scenario duration
        socket.setTimeout(function () {
            socket.close();
        }, 45000);
    });

    check(res, {
        'WebSocket status is 101': (r) => r && r.status === 101,
    });
}
