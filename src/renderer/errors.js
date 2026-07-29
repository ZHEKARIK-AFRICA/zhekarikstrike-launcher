export function errorMessage(error) {
    if (error == null) return '';
    if (typeof error === 'string') return error;
    if (error instanceof Error) return error.message;
    if (typeof error === 'object') {
        return error.message || error.details || JSON.stringify(error);
    }
    return String(error);
}

export function errorPresentation(error, contextKey, translate) {
    const technical = errorMessage(error);
    if (!contextKey && (typeof error === 'string' || error instanceof Error)) {
        return { friendly: technical, technical: '' };
    }
    const code = typeof error === 'object' && error ? error.code : null;
    const fallbackKey = code ? `errors.codes.${code}` : 'errors.codes.unknown';
    const friendlyKey = contextKey || fallbackKey;
    const translated = translate(friendlyKey);
    return {
        friendly: translated === friendlyKey ? translate('errors.codes.unknown') : translated,
        technical
    };
}
