const FIELD_SEPARATOR: u8 = 0x1f;
const FIELD_COUNT: usize = 10;

pub const LOG_FORMAT: &str =
    "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%D%x1f%s";

pub struct RawCommit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub refs: String,
    pub subject: String,
}

pub fn parse_record(record: &[u8]) -> Option<RawCommit> {
    let mut fields = record.splitn(FIELD_COUNT, |byte| *byte == FIELD_SEPARATOR);
    let mut next = || fields.next().map(|field| String::from_utf8_lossy(field));
    let hash = next()?.into_owned();
    let parents = next()?.split_whitespace().map(str::to_string).collect();
    let author_name = next()?.into_owned();
    let author_email = next()?.into_owned();
    // Git leaves the date empty if it cannot read the author, and the row must go on.
    let author_time = next()?.parse().unwrap_or(0);
    let committer_name = next()?.into_owned();
    let committer_email = next()?.into_owned();
    let commit_time = next()?.parse().unwrap_or(0);
    let refs = next()?.into_owned();
    let subject = next()?.into_owned();
    Some(RawCommit {
        hash,
        parents,
        author_name,
        author_email,
        author_time,
        committer_name,
        committer_email,
        commit_time,
        refs,
        subject,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPLETE: &[u8] =
        b"aaa\x1fbbb ccc\x1fAlice\x1falice@a.com\x1f1700000000\x1fBob\x1fbob@b.com\x1f1700000500\x1fHEAD -> refs/heads/main\x1fSubject";

    #[test]
    fn complete_record() {
        let raw = parse_record(COMPLETE).unwrap();
        assert_eq!(raw.hash, "aaa");
        assert_eq!(raw.parents, ["bbb", "ccc"]);
        assert_eq!(raw.author_name, "Alice");
        assert_eq!(raw.author_email, "alice@a.com");
        assert_eq!(raw.author_time, 1700000000);
        assert_eq!(raw.committer_name, "Bob");
        assert_eq!(raw.committer_email, "bob@b.com");
        assert_eq!(raw.commit_time, 1700000500);
        assert_eq!(raw.refs, "HEAD -> refs/heads/main");
        assert_eq!(raw.subject, "Subject");
    }

    #[test]
    fn subject_with_separator() {
        let raw =
            parse_record(b"aaa\x1f\x1fAlice\x1fa\x1f1\x1fAlice\x1fa\x1f1\x1f\x1ftwo\x1fparts")
                .unwrap();
        assert_eq!(raw.subject, "two\x1fparts");
        assert!(raw.parents.is_empty());
    }

    #[test]
    fn non_utf8_bytes() {
        let raw =
            parse_record(b"aaa\x1f\x1fAlice\xff\x1fa\x1f1\x1fBo\xfe\x1fb\x1f1\x1f\x1fcaf\xe9")
                .unwrap();
        assert_eq!(raw.author_name, "Alice\u{fffd}");
        assert_eq!(raw.committer_name, "Bo\u{fffd}");
        assert_eq!(raw.subject, "caf\u{fffd}");
    }

    #[test]
    fn unreadable_date_does_not_lose_the_row() {
        let raw = parse_record(b"aaa\x1f\x1f\x1f\x1f\x1f\x1f\x1f\x1f\x1fbroken author").unwrap();
        assert_eq!(raw.author_time, 0);
        assert_eq!(raw.commit_time, 0);
        assert_eq!(raw.subject, "broken author");
    }

    #[test]
    fn incomplete_record() {
        assert!(parse_record(b"aaa\x1fbbb").is_none());
    }
}
