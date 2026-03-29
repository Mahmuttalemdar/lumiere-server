import http from 'k6/http';
import { check, sleep } from 'k6';
import { register, authHeaders } from '../helpers/auth.js';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';

export const options = {
    scenarios: {
        history_load: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '10s', target: 50 },
                { duration: '30s', target: 200 },
                { duration: '10s', target: 0 },
            ],
        },
    },
    thresholds: {
        http_req_duration: ['p(99)<20'],
        http_req_failed: ['rate<0.01'],
    },
};

export function setup() {
    const users = [];
    for (let i = 0; i < 10; i++) {
        const user = register(
            `histtest_${Date.now()}_${i}`,
            `hist_${Date.now()}_${i}@test.com`,
            'testpassword123'
        );
        if (user) users.push(user);
    }

    if (users.length === 0) {
        throw new Error('No users could be registered for history test');
    }

    const serverRes = http.post(
        `${BASE_URL}/api/v1/servers`,
        JSON.stringify({ name: 'History Test Server' }),
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

    // Seed messages for history retrieval
    for (let i = 0; i < 100; i++) {
        http.post(
            `${BASE_URL}/api/v1/channels/${textChannel.id}/messages`,
            JSON.stringify({ content: `Seed message ${i}` }),
            { headers: authHeaders(users[0].access_token) }
        );
    }

    return { users, channelId: textChannel.id, serverId: server.id };
}

export default function (data) {
    const user = data.users[__VU % data.users.length];

    // Fetch latest messages (no cursor)
    const res = http.get(
        `${BASE_URL}/api/v1/channels/${data.channelId}/messages?limit=50`,
        { headers: authHeaders(user.access_token) }
    );

    check(res, {
        'status is 200': (r) => r.status === 200,
        'response time < 20ms': (r) => r.timings.duration < 20,
        'returns messages': (r) => {
            const body = JSON.parse(r.body);
            return Array.isArray(body) && body.length > 0;
        },
    });

    // Paginated fetch with cursor
    const messages = JSON.parse(res.body);
    if (messages.length > 0) {
        const lastId = messages[messages.length - 1].id;
        const pageRes = http.get(
            `${BASE_URL}/api/v1/channels/${data.channelId}/messages?limit=50&before=${lastId}`,
            { headers: authHeaders(user.access_token) }
        );

        check(pageRes, {
            'pagination status is 200': (r) => r.status === 200,
            'pagination response time < 20ms': (r) => r.timings.duration < 20,
        });
    }

    sleep(0.1);
}
