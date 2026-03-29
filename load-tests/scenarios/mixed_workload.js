import http from 'k6/http';
import { check, sleep, group } from 'k6';
import { register, login, authHeaders } from '../helpers/auth.js';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';

export const options = {
    scenarios: {
        mixed_realistic: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '15s', target: 50 },
                { duration: '60s', target: 200 },
                { duration: '30s', target: 200 },
                { duration: '15s', target: 0 },
            ],
        },
    },
    thresholds: {
        http_req_duration: ['p(95)<100', 'p(99)<200'],
        http_req_failed: ['rate<0.02'],
    },
};

export function setup() {
    const users = [];
    for (let i = 0; i < 10; i++) {
        const user = register(
            `mixed_${Date.now()}_${i}`,
            `mixed_${Date.now()}_${i}@test.com`,
            'testpassword123'
        );
        if (user) users.push(user);
    }

    if (users.length === 0) {
        throw new Error('No users could be registered for mixed workload test');
    }

    const serverRes = http.post(
        `${BASE_URL}/api/v1/servers`,
        JSON.stringify({ name: 'Mixed Workload Server' }),
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

    // Seed some messages
    for (let i = 0; i < 50; i++) {
        http.post(
            `${BASE_URL}/api/v1/channels/${textChannel.id}/messages`,
            JSON.stringify({ content: `Seed message ${i}` }),
            { headers: authHeaders(users[0].access_token) }
        );
    }

    return {
        users,
        serverId: server.id,
        channelId: textChannel.id,
    };
}

export default function (data) {
    const user = data.users[__VU % data.users.length];
    const headers = authHeaders(user.access_token);
    const roll = Math.random();

    if (roll < 0.70) {
        // 70% - Read messages (most common action)
        group('read_messages', function () {
            const res = http.get(
                `${BASE_URL}/api/v1/channels/${data.channelId}/messages?limit=50`,
                { headers }
            );
            check(res, {
                'read: status 200': (r) => r.status === 200,
                'read: response < 50ms': (r) => r.timings.duration < 50,
            });
        });
    } else if (roll < 0.90) {
        // 20% - Send messages
        group('send_message', function () {
            const res = http.post(
                `${BASE_URL}/api/v1/channels/${data.channelId}/messages`,
                JSON.stringify({ content: `Mixed workload msg ${Date.now()} VU ${__VU}` }),
                { headers }
            );
            check(res, {
                'send: status 201': (r) => r.status === 201,
                'send: response < 100ms': (r) => r.timings.duration < 100,
            });
        });
    } else if (roll < 0.95) {
        // 5% - Browse server info
        group('browse_server', function () {
            const res = http.get(
                `${BASE_URL}/api/v1/servers/${data.serverId}`,
                { headers }
            );
            check(res, {
                'server: status 200': (r) => r.status === 200,
            });

            const chRes = http.get(
                `${BASE_URL}/api/v1/servers/${data.serverId}/channels`,
                { headers }
            );
            check(chRes, {
                'channels: status 200': (r) => r.status === 200,
            });

            const membersRes = http.get(
                `${BASE_URL}/api/v1/servers/${data.serverId}/members?limit=100`,
                { headers }
            );
            check(membersRes, {
                'members: status 200': (r) => r.status === 200,
            });
        });
    } else {
        // 5% - Profile updates
        group('update_profile', function () {
            const res = http.patch(
                `${BASE_URL}/api/v1/users/@me`,
                JSON.stringify({ display_name: `LoadUser_${__VU}_${Date.now()}` }),
                { headers }
            );
            check(res, {
                'profile: status 200': (r) => r.status === 200,
            });
        });
    }

    sleep(Math.random() * 0.5 + 0.1);
}
