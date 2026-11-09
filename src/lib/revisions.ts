// `CopyRevisionNumberAction`: the full hashes of the selection, which comes newest first, from the
// oldest to the newest ("to be able to pass to native client in terminal"), joined by a space. The
// log, the history, the Changes window and the annotations copy with it.
export const revisionNumbersText = (hashes: string[]) => [...hashes].reverse().join(" ");
