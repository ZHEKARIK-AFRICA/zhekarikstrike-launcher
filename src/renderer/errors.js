export function errorMessage(error) {
    if (error == null) return '';
    if (typeof error === 'string') return error;
    if (error instanceof Error) return error.message;
    if (typeof error === 'object') {
        return error.message || error.details || JSON.stringify(error);
    }
    return String(error);
}
