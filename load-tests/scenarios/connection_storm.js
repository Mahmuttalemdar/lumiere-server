import http from 'k6/http';
import ws from 'k6/ws';
import { check, sleep } from 'k6';
import { Counter } from 'k6/metrics';
import { register, authHeaders } from '../helpers/auth.js';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const WS_URL = BASE_URL.replace('http://', 'ws://').replace('https://', 'wss://');

const successfulConnections = new Counter('storm_connections_success');
const failedConnections = new Counter('storm_connections_failed');

export const options = {
    scenarios: {
        // Phase 1: HTTP connection storm
        http_storm: {
            executor: 'shared-iterations',
            vus: 200,
            iterations: 1000,
            maxDuration: '10s',
            exec: 'httpStorm',
        },
        // Phase 2: WebSocket connection storm
        ws_storm: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '10s', target: 1000 },
                { duration: '20s', target: 1000 },
                { duration: '5s', target: 0 },
            ],
            startTime: '15s',
            exec: 'wsStorm',
        },
    },
    thresholds: {
        storm_connections_success: ['count>800'],
        storm_connections_failed: ['count<100'],
        http_req_duration: ['p(95)<500'],
    },
};

export function setup() {
    // Create users for the storm
    const users = [];
    for (let i = 0; i < 20; i++) {
        const user = register(
            `storm_${Date.now()}_${i}`,
            `storm_${Date.now()}_${i}@test.com`,
            'testpassword123'
        );
        if (user) users.push(user);
    }

    if (users.length === 0) {
        throw new Error('No users could be registered for storm test');
    }

    // Create a server and channel for HTTP storm
    const serverRes = http.post(
        `${BASE_URL}/api/v1/servers`,
        JSON.stringify({ name: 'Storm Test Server' }),
        { headers: authHeaders(users[0].access_token) }
    );
    const server = JSON.parse(serverRes.body);

    const channelsRes = http.get(
        `${BASE_URL}/api/v1/servers/${server.id}/channels`,
        { headers: authHeaders(users[0].access_token) }
    );
    const channels = JSON.parse(channelsRes.body);
    const textChannel = channels.find(c => c.type === 0);

    return {
        users,
        serverId: server.id,
        channelId: textChannel.id,
    };
}

export function httpStorm(data) {
    const user = data.users[__VU % data.users.length];
    const headers = authHeaders(user.access_token);

    // Simulate a user opening the app: fetch server, channels, messages
    const batch = http.batch([
        ['GET', `${BASE_URL}/api/v1/servers/${data.serverId}`, null, { headers }],
        ['GET', `${BASE_URL}/api/v1/servers/${data.serverId}/channels`, null, { headers }],
        ['GET', `${BASE_URL}/api/v1/channels/${data.channelId}/messages?limit=50`, null, { headers }],
        ['GET', `${BASE_URL}/api/v1/users/@me`, null, { headers }],
    ]);

    for (const res of batch) {
        check(res, {
            'http storm: status 200': (r) => r.status === 200,
        });
    }
}

export function wsStorm(data) {
    const user = data.users[__VU % data.users.length];
    const token = user.access_token;

    const url = `${WS_URL}/gateway`;

    const res = ws.connect(url, {}, function (socket) {
        socket.on('open', function () {
            successfulConnections.add(1);

            socket.send(JSON.stringify({
                op: 1,
                d: { token: token },
            }));
        });

        socket.on('error', function () {
            failedConnections.add(1);
        });

        // Hold connection briefly then disconnect
        socket.setTimeout(function () {
            socket.close();
        }, 15000);
    });

    check(res, {
        'ws storm: connected': (r) => r && r.status === 101,
    });
}
