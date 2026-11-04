//! Synthetic cover letters: US and Canadian business letters, Europass, German DIN and Albanian,
//! with varied blocks, held-out salutations, closings and subjects, wrapped addresses and noise.

use super::pools::STRAY_WORDS;
use super::resume::{Region, phone, place};
use super::{
    Align, CorpusStyle, Item, Page, Person, Slot, SplitMix64, company, lang_data, lang_pools,
    pick_held, wrap,
};
use crate::profiles::extract::lines::labels::LetterLineRole as Role;

fn fill(template: &str, role: &str, company: &str, years: i32) -> String {
    template
        .replace("{role}", role)
        .replace("{company}", company)
        .replace("{years}", &years.to_string())
}

/// The letter date written in the style of the language.
fn date_text(style: CorpusStyle, months: &[&str; 12], city: &str, rng: &mut SplitMix64) -> String {
    let month = usize::try_from(rng.range(1, 12) - 1).unwrap_or(0);
    let day = rng.range(1, 28);
    let name = months[month];
    let plain = match style {
        CorpusStyle::LetterUs => match rng.below(3) {
            0 => format!("{name} {day}, 2025"),
            1 => format!("{day} {name} 2025"),
            _ => format!("2025-{:02}-{day:02}", month + 1),
        },
        CorpusStyle::LetterGermanDin => format!("{day}. {name} 2025"),
        _ => format!("{day} {name} 2025"),
    };
    match style {
        CorpusStyle::LetterGermanDin | CorpusStyle::LetterAlbanian => format!("{city}, {plain}"),
        _ => plain,
    }
}

/// Address lines of a block: the street, then the city line, which is wrapped after a comma (and
/// flagged as a continuation) when the block is left-aligned.
fn address_lines(
    items: &mut Vec<Item>,
    address: &str,
    role: Role,
    slot: Slot,
    size: i32,
    rng: &mut SplitMix64,
    first_gap: bool,
) {
    let (street, city) = address.split_once(", ").unwrap_or((address, ""));
    let wrapped = slot == Slot::Main && !city.is_empty() && rng.chance(35);
    let street_item = Item::letter(
        if wrapped {
            format!("{street},")
        } else {
            street.to_owned()
        },
        role,
        slot,
    )
    .size(size);
    items.push(if first_gap {
        street_item.gap()
    } else {
        street_item
    });
    if !city.is_empty() {
        let city_item = Item::letter(city, role, slot).size(size);
        items.push(if wrapped {
            city_item.continues(" ")
        } else {
            city_item
        });
    }
}

/// Builds the page and the items of one cover letter.
pub(super) fn build(style: CorpusStyle, rng: &mut SplitMix64) -> (Page, Vec<Item>) {
    let language = style.language();
    let (region, european) = match style {
        CorpusStyle::LetterUs => (Region::Us, false),
        CorpusStyle::LetterCanadaFrench => (Region::CaFr, false),
        CorpusStyle::LetterEuropass | CorpusStyle::LetterGermanDin => (Region::De, true),
        _ => (Region::Sq, true),
    };
    let data = lang_data(language);
    let pools = lang_pools(language);
    let page = if european {
        Page::single_column(59500, 84200)
    } else {
        Page::single_column(61200, 79200)
    };
    let body = rng.range(18, 22) * 50;
    let din = style == CorpusStyle::LetterGermanDin;
    let right_block = din || rng.chance(20);
    let block = if right_block {
        Slot::Full(Align::Right)
    } else {
        Slot::Main
    };
    let sender = Person::new(rng, data);
    let contact = Person::new(rng, data);
    let home = place(rng, region);
    let office = place(rng, region);
    let title = *rng.pick(data.letter_titles);
    let role = pick_held(rng, data.titles, pools.held_titles);
    let employer = company(rng, language);
    let years = rng.range(3, 12);
    let city = home.short.split(", ").next().unwrap_or("").to_owned();
    let mut items: Vec<Item> = Vec::new();

    // Sender block.
    let name_size = rng.range(12, 20) * 100;
    items.push(
        Item::letter(sender.full_name(), Role::SenderName, block)
            .size(name_size)
            .bold(),
    );
    let mut parts: Vec<&str> = vec!["email"];
    for (kind, percent) in [("address", 85), ("phone", 85), ("url", 35)] {
        if rng.chance(percent) {
            parts.push(kind);
        }
    }
    for index in (1..parts.len()).rev() {
        let other = rng.below(index + 1);
        parts.swap(index, other);
    }
    for part in parts {
        match part {
            "address" => address_lines(
                &mut items,
                &home.address,
                Role::SenderContact,
                block,
                body,
                rng,
                false,
            ),
            "phone" => {
                let number = phone(rng, region, style == CorpusStyle::LetterEuropass);
                items.push(Item::letter(number, Role::SenderContact, block).size(body));
            }
            "url" => items.push(
                Item::letter(sender.url_short.clone(), Role::SenderContact, block).size(body),
            ),
            _ => items
                .push(Item::letter(sender.email.clone(), Role::SenderContact, block).size(body)),
        }
    }

    // Date and recipient, in either order.
    let date_slot = if din || rng.chance(35) {
        Slot::Full(Align::Right)
    } else {
        Slot::Main
    };
    let date = Item::letter(
        date_text(style, &data.months_long, &city, rng),
        Role::Date,
        date_slot,
    )
    .size(body)
    .gap();
    let mut recipient: Vec<Item> = Vec::new();
    let named = rng.chance(85);
    if named {
        let name = format!("{title} {}", contact.full_name());
        let text = if rng.chance(30) {
            format!("{} {name}", data.letter_recipient_marker)
        } else {
            name
        };
        recipient.push(
            Item::letter(text, Role::Recipient, Slot::Main)
                .size(body)
                .gap(),
        );
    }
    if style != CorpusStyle::LetterGermanDin && rng.chance(70) {
        let department = match style {
            CorpusStyle::LetterCanadaFrench => "Ressources humaines",
            CorpusStyle::LetterAlbanian => "Zyra e Burimeve Njerëzore",
            _ => "Hiring Manager",
        };
        recipient.push(Item::letter(department, Role::Recipient, Slot::Main).size(body));
    }
    recipient.push(Item::letter(employer.clone(), Role::Recipient, Slot::Main).size(body));
    address_lines(
        &mut recipient,
        &office.address,
        Role::Recipient,
        Slot::Main,
        body,
        rng,
        false,
    );
    if let Some(first) = recipient.first_mut() {
        first.gap = true;
    }
    let date_first = !(din || style == CorpusStyle::LetterEuropass) ^ rng.chance(20);
    if date_first {
        items.push(date);
        items.extend(recipient);
    } else {
        items.extend(recipient);
        items.push(date);
    }

    // Subject, salutation, body.
    if rng.chance(80) {
        let subject = fill(data.letter_subject, role, &employer, years);
        let text = if rng.chance(33) {
            fill(
                pools.held_subjects[rng.below(pools.held_subjects.len())],
                role,
                &employer,
                years,
            )
        } else {
            let marker = if style == CorpusStyle::LetterUs && rng.chance(30) {
                "Re:"
            } else {
                data.letter_subject_marker
            };
            format!("{marker} {subject}")
        };
        let item = Item::letter(text, Role::Subject, Slot::Main)
            .size(body)
            .gap();
        items.push(match rng.below(3) {
            0 => item.bold(),
            1 => item.italic(),
            _ => item,
        });
    }
    let last = &contact.last;
    let standard = match style {
        CorpusStyle::LetterCanadaFrench => {
            if rng.chance(50) {
                "Madame, Monsieur,".to_owned()
            } else {
                format!("{title} {last},")
            }
        }
        CorpusStyle::LetterGermanDin => {
            if rng.chance(15) {
                "Sehr geehrte Damen und Herren,".to_owned()
            } else {
                let ending = if title == "Frau" { "e" } else { "er" };
                format!("Sehr geehrt{ending} {title} {last},")
            }
        }
        CorpusStyle::LetterAlbanian => {
            let lead = if title == "Znj." { "E" } else { "I" };
            format!("{lead} nderuar {title} {last},")
        }
        _ => match rng.below(10) {
            0 => "To whom it may concern,".to_owned(),
            1 => "Dear Hiring Manager,".to_owned(),
            _ => format!("Dear {title} {last},"),
        },
    };
    let salutation = if rng.chance(33) {
        pools.held_salutations[rng.below(pools.held_salutations.len())]
            .replace("{title}", title)
            .replace("{last}", last)
    } else {
        standard
    };
    items.push(
        Item::letter(salutation, Role::Salutation, Slot::Main)
            .size(body)
            .gap(),
    );
    let mut sentences: Vec<String> = Vec::new();
    sentences.push(fill(
        data.letter_openings[rng.below(data.letter_openings.len())],
        role,
        &employer,
        years,
    ));
    for middle in data.letter_middles {
        sentences.push(fill(middle, role, &employer, years));
    }
    let closing_sentence = fill(
        data.letter_closings[rng.below(data.letter_closings.len())],
        role,
        &employer,
        years,
    );
    let paragraphs = usize::try_from(rng.range(2, 4)).unwrap_or(2);
    let mut texts: Vec<String> = Vec::new();
    let per = sentences
        .len()
        .div_ceil(paragraphs.saturating_sub(1).max(1));
    for chunk in sentences.chunks(per) {
        texts.push(chunk.join(" "));
    }
    texts.push(closing_sentence);
    let width = page.main_capacity(body);
    for paragraph in texts {
        for (position, line) in wrap(&paragraph, width).into_iter().enumerate() {
            let item = Item::letter(line, Role::Body, Slot::Main).size(body);
            items.push(if position == 0 { item.gap() } else { item });
        }
    }

    // Closing, signature, enclosure, stray word.
    let standard_closing = match style {
        CorpusStyle::LetterUs => *rng.pick(&["Sincerely,", "Kind regards,", "Best regards,"]),
        CorpusStyle::LetterEuropass => *rng.pick(&["Yours faithfully,", "Kind regards,"]),
        CorpusStyle::LetterCanadaFrench => *rng.pick(&[
            "Cordialement,",
            "Veuillez agréer, Madame, Monsieur, mes salutations distinguées.",
            "Je vous prie d'agréer, Madame, Monsieur, mes salutations distinguées.",
        ]),
        CorpusStyle::LetterGermanDin => *rng.pick(&[
            "Mit freundlichen Grüßen",
            "Mit besten Grüßen",
            "Freundliche Grüße",
        ]),
        _ => *rng.pick(&["Me respekt,", "Me nderim,", "Faleminderit,"]),
    };
    let closing = if rng.chance(33) {
        *rng.pick(pools.held_closings)
    } else {
        standard_closing
    };
    items.push(
        Item::letter(closing, Role::Closing, Slot::Main)
            .size(body)
            .gap(),
    );
    items.push(
        Item::letter(sender.full_name(), Role::Signature, Slot::Main)
            .size(body)
            .gap()
            .italic(),
    );
    if rng.chance(40) {
        items.push(Item::letter(role, Role::Signature, Slot::Main).size(body));
    }
    if rng.chance(50) {
        items.push(
            Item::letter(data.letter_enclosure, Role::Other, Slot::Main)
                .size(body)
                .gap(),
        );
    }
    if rng.chance(25) {
        items.push(Item::letter(*rng.pick(STRAY_WORDS), Role::Other, Slot::Main).size(body));
    }
    (page, items)
}
