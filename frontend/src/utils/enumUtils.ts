/**
 * Converts a numeric enum into an array of option objects,
 * each containing a numeric value and its corresponding enum key as text.
 * This works for TypeScript numeric enums due to the reverse mapping.
 */
export function enumToOptions<E extends Record<string, number | string>>(enumObj: E): { value: number; name: string }[] {
    return Object.values(enumObj)
        .filter((v): v is number => typeof v === 'number')
        .map((value) => ({
            value,
            name: formatEnumValue(enumObj[value as unknown as keyof E] as string),
        }))
}

export function formatEnumValue(value: string): string {
    return value
        .replace(/_/g, ' ') // Replace all underscores with spaces
        .replace(/([a-z])([A-Z])/g, '$1 $2') // Split camelCase
        .replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2') // Split acronym followed by camelCase
        .split(/\s+/) // Split into words by any whitespace
        .map(word => /^[A-Z]+$/.test(word) ? word : word.toLowerCase()) // Lowercase non-acronym words
        .join(' ')
        .replace(/bit rate/g, 'bitrate') // Replace all occurrences of "bit rate"
        .replace(/(^\s*| )(\S)/g, (_, space, char) => space + char.toUpperCase()) // Capitalize first letter of each word
}