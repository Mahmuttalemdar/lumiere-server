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
    const user = register(
        `loadtest_${Date.now()}`,
        `load_${Date.now()}@test.com`,
        'testpassword123'
    );

    const serverRes = http.post(
        `${BASE_URL}/api/v1/servers`,
        JSON.stringify({ name: 'Load Test Server' }),
        { headers: authHeaders(user.access_token) }
    );
    const server = JSON.parse(serverRes.body);

    const channelsRes = http.get(
        `${BASE_URL}/api/v1/servers/${server.id}/channels`,
        { headers: authHeaders(user.access_token) }
    );
    const channels = JSON.parse(channelsRes.body);
    const textChannel = channels.find(c => c.type === 0);

    return { token: user.access_token, channelId: textChannel.id };
}

export default function (data) {
    const res = http.post(
        `${BASE_URL}/api/v1/channels/${data.channelId}/messages`,
        JSON.stringify({ content: `Load test message ${Date.now()} from VU ${__VU}` }),
        { headers: authHeaders(data.token) }
    );

    check(res, {
        'status is 201': (r) => r.status === 201,
        'response time < 50ms': (r) => r.timings.duration < 50,
    });
}
