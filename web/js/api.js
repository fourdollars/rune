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
        const data = await response.json();
        if (!data.ok && data.error) globalThis.addSystemMessage('Error: ' + data.error);
        return data;
    } catch (error) {
        console.error('API error:', error);
        globalThis.addSystemMessage('Error: ' + error.message);
        return { ok: false, error: error.message };
    }
};

globalThis.api = api;
