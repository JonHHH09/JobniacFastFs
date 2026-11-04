//! The line-role label sets of resumes and cover letters. The names are stable: a model file and
//! a labelled dataset refer to them.

/// The role of one line of a resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResumeLineRole {
    /// The person's name.
    Name,
    /// An email, phone, address, link or other contact detail.
    Contact,
    /// A section heading.
    SectionHeading,
    /// A date or date range.
    DateRange,
    /// A job title, degree or project name.
    Title,
    /// An employer, school or other organization.
    Organization,
    /// A place (city, region).
    Location,
    /// A bullet point.
    Bullet,
    /// Running text: summary, descriptions, skill and language lines.
    Body,
    /// Anything else (footer, page number, separator).
    Other,
}

impl ResumeLineRole {
    /// Every role, in label order.
    pub const ALL: [ResumeLineRole; 10] = [
        ResumeLineRole::Name,
        ResumeLineRole::Contact,
        ResumeLineRole::SectionHeading,
        ResumeLineRole::DateRange,
        ResumeLineRole::Title,
        ResumeLineRole::Organization,
        ResumeLineRole::Location,
        ResumeLineRole::Bullet,
        ResumeLineRole::Body,
        ResumeLineRole::Other,
    ];

    /// The stable name of the role.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ResumeLineRole::Name => "name",
            ResumeLineRole::Contact => "contact",
            ResumeLineRole::SectionHeading => "section_heading",
            ResumeLineRole::DateRange => "date_range",
            ResumeLineRole::Title => "title",
            ResumeLineRole::Organization => "organization",
            ResumeLineRole::Location => "location",
            ResumeLineRole::Bullet => "bullet",
            ResumeLineRole::Body => "body",
            ResumeLineRole::Other => "other",
        }
    }

    /// The role with this stable name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.name() == name)
    }
}

/// The role of one line of a cover letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LetterLineRole {
    /// The sender's name in the letterhead.
    SenderName,
    /// The sender's address, phone, email or link.
    SenderContact,
    /// The letter date (with the place, in German letters).
    Date,
    /// The recipient's name, title, company or address.
    Recipient,
    /// The subject line.
    Subject,
    /// The salutation.
    Salutation,
    /// A line of the letter body.
    Body,
    /// The complimentary closing.
    Closing,
    /// The signature name below the closing.
    Signature,
    /// Anything else (enclosure note, page number).
    Other,
}

impl LetterLineRole {
    /// Every role, in label order.
    pub const ALL: [LetterLineRole; 10] = [
        LetterLineRole::SenderName,
        LetterLineRole::SenderContact,
        LetterLineRole::Date,
        LetterLineRole::Recipient,
        LetterLineRole::Subject,
        LetterLineRole::Salutation,
        LetterLineRole::Body,
        LetterLineRole::Closing,
        LetterLineRole::Signature,
        LetterLineRole::Other,
    ];

    /// The stable name of the role.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LetterLineRole::SenderName => "sender_name",
            LetterLineRole::SenderContact => "sender_contact",
            LetterLineRole::Date => "date",
            LetterLineRole::Recipient => "recipient",
            LetterLineRole::Subject => "subject",
            LetterLineRole::Salutation => "salutation",
            LetterLineRole::Body => "body",
            LetterLineRole::Closing => "closing",
            LetterLineRole::Signature => "signature",
            LetterLineRole::Other => "other",
        }
    }

    /// The role with this stable name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.name() == name)
    }
}

/// The label of one line of either kind of document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LineLabel {
    /// A resume line role.
    Resume(ResumeLineRole),
    /// A cover-letter line role.
    Letter(LetterLineRole),
}

impl LineLabel {
    /// The stable name of the role.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LineLabel::Resume(role) => role.name(),
            LineLabel::Letter(role) => role.name(),
        }
    }
}
