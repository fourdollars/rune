export async function api(endpoint, body, method) {
    const requestMethod = method || (body !== undefined ? 'POST' : 'GET');
    const options = {
        method: requestMethod,
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
    };
    if (body !== undefined && requestMethod !== 'GET' && requestMethod !== 'DELETE') {
        options.body = JSON.stringify(body);
    }
    try {
        const response = await fetch('/api/' + endpoint, options);
        const text = await response.text();
        let data = {};
        if (text && text.trim().length > 0) {
            try {
                data = JSON.parse(text);
            } catch (e) {
                data = { ok: false, error: text };
            }
        }
        if (!response.ok) {
            data.ok = false;
            if (!data.error) {
                data.error = `HTTP ${response.status} ${response.statusText || 'Error'}`;
            }
        }
        if (!data.ok && data.error && typeof globalThis.addSystemMessage === 'function') {
            globalThis.addSystemMessage('Error: ' + data.error);
        }
        return data;
    } catch (error) {
        console.error('API error:', error);
        if (typeof globalThis.addSystemMessage === 'function') {
            globalThis.addSystemMessage('Error: ' + error.message);
        }
        return { ok: false, error: error.message };
    }
};

globalThis.api = api;
