import http from 'k6/http';
import { check } from 'k6';
import { register, authHeaders } from '../helpers/auth.js';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';

export const options = {
    scenarios: {
        message_flood: {
            executor: 'constant-arrival-rate',
            rate: 1000,
            timeUnit: '1s',
            duration: '30s',
            preAllocatedVUs: 100,
            maxVUs: 200,
        },
    },
    thresholds: {
        http_req_duration: ['p(99)<50'],
        http_req_failed: ['rate<0.01'],
    },
};

export function setup() {
    const users = [];
    for (let i = 0; i < 10; i++) {
        const user = register(
            `loadtest_${Date.now()}_${i}`,
            `load_${Date.now()}_${i}@test.com`,
            'testpassword123'
        );
        if (user) users.push(user);
    }

    if (users.length === 0) {
        throw new Error('No users could be registered for load test');
    }

    const serverRes = http.post(
        `${BASE_URL}/api/v1/servers`,
        JSON.stringify({ name: 'Load Test Server' }),
        { headers: authHeaders(users[0].access_token) }
    );
    const server = JSON.parse(serverRes.body);

    const channelsRes = http.get(
        `${BASE_URL}/api/v1/servers/${server.id}/channels`,
        { headers: authHeaders(users[0].access_token) }
    );
    const channels = JSON.parse(channelsRes.body);
    const textChannel = channels.find(c => c.type === 0);

    // Have all other users join the server
    for (let i = 1; i < users.length; i++) {
        http.post(
            `${BASE_URL}/api/v1/servers/${server.id}/join`,
            null,
            { headers: authHeaders(users[i].access_token) }
        );
    }

    return { users, channelId: textChannel.id, serverId: server.id };
}

export default function (data) {
    const user = data.users[__VU % data.users.length];

    const res = http.post(
        `${BASE_URL}/api/v1/channels/${data.channelId}/messages`,
        JSON.stringify({ content: `Load test message ${Date.now()} from VU ${__VU}` }),
        { headers: authHeaders(user.access_token) }
    );

    check(res, {
        'status is 201': (r) => r.status === 201,
        'response time < 50ms': (r) => r.timings.duration < 50,
    });
}
