import http from 'k6/http';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';

export function register(username, email, password) {
    const res = http.post(`${BASE_URL}/api/v1/auth/register`, JSON.stringify({
        username, email, password
    }), { headers: { 'Content-Type': 'application/json' } });

    if (res.status !== 201) {
        console.error(`Register failed: ${res.status} ${res.body}`);
        return null;
    }
    return JSON.parse(res.body);
}

export function login(email, password) {
    const res = http.post(`${BASE_URL}/api/v1/auth/login`, JSON.stringify({
        email, password
    }), { headers: { 'Content-Type': 'application/json' } });

    if (res.status !== 200) {
        console.error(`Login failed: ${res.status} ${res.body}`);
        return null;
    }
    return JSON.parse(res.body);
}

export function authHeaders(token) {
    return {
        'Authorization': `Bearer ${token}`,
        'Content-Type': 'application/json',
    };
}
