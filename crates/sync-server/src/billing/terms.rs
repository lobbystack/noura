//! The mail about plans that the terms promise (§26 Billing): the contract
//! after the first payment, the confirmations of a cancellation, a refund
//! and a withdrawal, renewal reminders and price-change notices. Each
//! message comes in the account's language; a customer in Québec gets it
//! in French, with the English after it.

use sync_store::{AccountPlanRow, BillingMail, ConsentRow};

use super::{Merchant, PaidOrder};
use crate::accounts::lang::Lang;
use crate::accounts::plan::plan_name;

/// The version of the consent text the upgrade page shows. Change it with
/// the text.
pub const CONSENT_VERSION: &str = "2026-11-27";

/// The languages of one message: one, or French then English.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MailLang {
    /// The language the message starts in.
    pub first: Lang,
    /// Whether the English follows the French.
    pub both: bool,
}

impl MailLang {
    /// The languages for an account that reads its pages in `stored`, and
    /// whose billing address may be in Québec.
    pub fn for_account(stored: Option<&str>, quebec: bool) -> Self {
        if quebec {
            return Self {
                first: Lang::Fr,
                both: true,
            };
        }
        Self {
            first: stored.and_then(Lang::from_code).unwrap_or_default(),
            both: false,
        }
    }

    /// One message in these languages, each part written by `write`.
    pub fn compose(self, recipient: &str, write: impl Fn(Lang) -> (String, String)) -> BillingMail {
        let (subject, body) = write(self.first);
        if !self.both {
            return BillingMail {
                recipient: recipient.to_owned(),
                subject,
                body,
            };
        }
        let (english_subject, english) = write(Lang::En);
        BillingMail {
            recipient: recipient.to_owned(),
            subject: format!("{subject} / {english_subject}"),
            body: format!("{body}\n* * *\n\n{english}"),
        }
    }
}

/// What every message about a plan needs to know.
pub struct Context<'a> {
    /// Who sells the plans.
    pub merchant: &'a Merchant,
    /// The server's public origin, such as `https://sync.example.com`.
    pub origin: &'a str,
}

impl Context<'_> {
    fn account_page(&self) -> String {
        format!("{}/account", self.origin)
    }

    /// How to cancel, in one sentence.
    fn how_to_cancel(&self, lang: Lang) -> String {
        match lang {
            Lang::En => format!(
                "To cancel, sign in at {} and choose Cancel plan, or email {}.",
                self.account_page(),
                self.merchant.email
            ),
            Lang::Fr => format!(
                "Pour résilier, connectez-vous à {} et choisissez Résilier le forfait, ou écrivez à {}.",
                self.account_page(),
                self.merchant.email
            ),
        }
    }

    /// The closing lines: the terms and who sells the plan.
    fn footer(&self, lang: Lang) -> String {
        match lang {
            Lang::En => format!(
                "Terms: {}\nSeller: {}\n",
                self.merchant.terms(lang),
                self.merchant.contact()
            ),
            Lang::Fr => format!(
                "Conditions : {}\nVendeur : {}\n",
                self.merchant.terms(lang),
                self.merchant.contact()
            ),
        }
    }
}

/// An amount in the currency's smallest unit, with its currency's code,
/// as `lang` writes it.
pub fn money(amount: u64, currency: &str, lang: Lang) -> String {
    let code = currency.to_ascii_uppercase();
    let (units, cents) = (amount / 100, amount % 100);
    match lang {
        Lang::En => format!("{code} {units}.{cents:02}"),
        Lang::Fr => format!("{units},{cents:02} {code}"),
    }
}

/// A Unix time's UTC date, as `lang` writes it.
pub fn date(time: u64, lang: Lang) -> String {
    let Ok(time) = jiff::Timestamp::from_second(i64::try_from(time).unwrap_or(i64::MAX)) else {
        return String::new();
    };
    let day = time.to_zoned(jiff::tz::TimeZone::UTC).date();
    match lang {
        Lang::En => day.strftime("%B %-d, %Y").to_string(),
        Lang::Fr => {
            const MONTHS: [&str; 12] = [
                "janvier",
                "février",
                "mars",
                "avril",
                "mai",
                "juin",
                "juillet",
                "août",
                "septembre",
                "octobre",
                "novembre",
                "décembre",
            ];
            let month = MONTHS[usize::try_from(day.month() - 1).unwrap_or(0)];
            let first = if day.day() == 1 { "er" } else { "" };
            format!("{}{first} {month} {}", day.day(), day.year())
        }
    }
}

/// `month` or `year` as a period of the plan, in `lang`.
fn period(interval: Option<&str>, lang: Lang) -> &'static str {
    match (interval == Some("year"), lang) {
        (true, Lang::En) => "year",
        (true, Lang::Fr) => "année",
        (false, Lang::En) => "month",
        (false, Lang::Fr) => "mois",
    }
}

/// The contract, mailed after a subscription's first payment.
pub fn contract(
    context: &Context<'_>,
    langs: MailLang,
    recipient: &str,
    order: &PaidOrder,
    consent: Option<&ConsentRow>,
) -> BillingMail {
    let quebec =
        order.terms.country.as_deref() == Some("CA") && order.terms.region.as_deref() == Some("QC");
    let interval = order.terms.interval.as_deref();
    let yearly = interval == Some("year");
    let price = order.terms.amount.unwrap_or(order.net);
    langs.compose(recipient, |lang| {
        let money = |amount| money(amount, &order.currency, lang);
        let when = |time| date(time, lang);
        let plan = plan_name(&order.plan, lang);
        let every = period(interval, lang);
        let renews = order.period_end.map(when).unwrap_or_default();
        let name = order.name.clone().unwrap_or_default();
        let address = order.address.join(", ");
        let mut text = String::new();
        let subject;
        match lang {
            Lang::En => {
                subject = "Your Noura Sync contract".to_owned();
                text.push_str("This email is your contract for Noura Sync. Keep it, or print it.\n\n");
                text.push_str(&format!("Customer: {name}\n"));
                if !address.is_empty() {
                    text.push_str(&format!("Billing address: {address}\n"));
                }
                text.push_str(&format!("Date: {}\n", when(order.paid_at)));
                text.push_str(&format!("Plan: {plan}\n"));
                text.push_str(&format!("Price: {} a {every}, before tax\n", money(price)));
                text.push_str(&format!("Tax: {}\n", money(order.tax)));
                text.push_str(&format!("Total charged: {}\n", money(order.total)));
                text.push_str(&format!(
                    "Billing: every {every}, in advance. Next renewal: {renews}.\n"
                ));
                if quebec {
                    let monthly = if yearly { price.div_ceil(12) } else { price };
                    text.push_str(&format!("Monthly rate: {}\n", money(monthly)));
                    text.push_str(&format!("Current period ends: {renews}\n"));
                }
                text.push('\n');
                text.push_str(&format!(
                    "Automatic renewal: the plan renews every {every} at the price in effect until you cancel, and you authorize a charge for each renewal."
                ));
                if yearly {
                    text.push_str(" We email you twice before each yearly renewal.");
                }
                if quebec && yearly {
                    text.push_str(" After its first year, the plan continues with no fixed end date, billed a year in advance.");
                }
                text.push_str("\n\n");
                text.push_str(&context.how_to_cancel(lang));
                if quebec {
                    text.push_str(" Your cancellation takes effect on the day you send it, or on a later date you choose, and we refund the unused part of the period with no fee.");
                } else {
                    text.push_str(" The plan stays active until the end of the period you paid for, then your account moves to the Free plan.");
                }
                text.push_str("\n\n");
                text.push_str("Refunds: you can get a full refund of any charge, tax included, if you ask within 14 days of it. Choose Refund last payment on your account page, or email us. A refund ends the period it paid for, and your account moves to the Free plan.\n\n");
                if let Some(consent) = consent {
                    text.push_str(&format!(
                        "Your consent: on {}, you agreed to the automatic renewal and asked us to start your subscription right away, during the 14-day withdrawal period. If you're a consumer in the EU or the UK, you can still withdraw within 14 days of your purchase with Withdraw from contract here on your account page, and we refund everything you paid.\n\n",
                        when(consent.accepted_at)
                    ));
                }
            }
            Lang::Fr => {
                subject = "Votre contrat Noura Sync".to_owned();
                text.push_str("Ce courriel est votre contrat pour Noura Sync. Conservez-le ou imprimez-le.\n\n");
                text.push_str(&format!("Client : {name}\n"));
                if !address.is_empty() {
                    text.push_str(&format!("Adresse de facturation : {address}\n"));
                }
                text.push_str(&format!("Date : {}\n", when(order.paid_at)));
                text.push_str(&format!("Forfait : {plan}\n"));
                text.push_str(&format!("Prix : {} par {every}, avant taxes\n", money(price)));
                text.push_str(&format!("Taxes : {}\n", money(order.tax)));
                text.push_str(&format!("Total facturé : {}\n", money(order.total)));
                text.push_str(&format!(
                    "Facturation : chaque {every}, d\u{2019}avance. Prochain renouvellement : {renews}.\n"
                ));
                if quebec {
                    let monthly = if yearly { price.div_ceil(12) } else { price };
                    text.push_str(&format!("Tarif mensuel : {}\n", money(monthly)));
                    text.push_str(&format!("Fin de la période en cours : {renews}\n"));
                }
                text.push('\n');
                text.push_str(&format!(
                    "Renouvellement automatique : le forfait se renouvelle chaque {every} au prix en vigueur jusqu\u{2019}à ce que vous le résiliiez, et vous autorisez le paiement de chaque renouvellement."
                ));
                if yearly {
                    text.push_str(" Nous vous écrivons deux fois avant chaque renouvellement annuel.");
                }
                if quebec && yearly {
                    text.push_str(" Après sa première année, le forfait se poursuit pour une durée indéterminée, facturé un an d\u{2019}avance.");
                }
                text.push_str("\n\n");
                text.push_str(&context.how_to_cancel(lang));
                if quebec {
                    text.push_str(" La résiliation prend effet le jour où vous l\u{2019}envoyez, ou à une date ultérieure de votre choix, et nous vous remboursons la partie non utilisée de la période, sans indemnité.");
                } else {
                    text.push_str(" Le forfait reste actif jusqu\u{2019}à la fin de la période payée, puis votre compte passe au forfait Gratuit.");
                }
                text.push_str("\n\n");
                text.push_str("Remboursements : vous pouvez obtenir le remboursement complet de tout paiement, taxes comprises, si vous le demandez dans les 14 jours qui suivent. Choisissez Rembourser le dernier paiement sur la page de votre compte, ou écrivez-nous. Un remboursement met fin à la période payée, et votre compte passe au forfait Gratuit.\n\n");
                if let Some(consent) = consent {
                    text.push_str(&format!(
                        "Votre consentement : le {}, vous avez accepté le renouvellement automatique et demandé que l\u{2019}abonnement commence immédiatement, pendant le délai de rétractation de 14 jours. Si vous êtes un consommateur dans l\u{2019}Union européenne ou au Royaume-Uni, vous pouvez encore vous rétracter dans les 14 jours suivant l\u{2019}achat avec Se rétracter du contrat ici sur la page de votre compte, et nous vous remboursons tout ce que vous avez payé.\n\n",
                        when(consent.accepted_at)
                    ));
                }
            }
        }
        text.push_str(&context.footer(lang));
        text.push_str(lang.pick(
            "Polar processes the payment as merchant of record and sends your receipt.\n",
            "Polar traite le paiement à titre de marchand officiel et vous envoie le reçu.\n",
        ));
        (subject, text)
    })
}

/// The confirmation of a cancellation that takes effect when the period
/// ends.
pub fn canceled_at_period_end(
    context: &Context<'_>,
    langs: MailLang,
    recipient: &str,
    row: &AccountPlanRow,
) -> BillingMail {
    langs.compose(recipient, |lang| {
        let plan = plan_name(&row.plan, lang);
        let ends = row.period_end.map(|t| date(t, lang)).unwrap_or_default();
        let text = match lang {
            Lang::En => format!(
                "You cancelled your {plan} plan. It stays active until {ends}. Your account then moves to the Free plan, and we don't charge you again.\n\nTo keep the plan, choose Manage billing at {} before then.\n\n{}",
                context.account_page(),
                context.footer(lang)
            ),
            Lang::Fr => format!(
                "Vous avez résilié votre forfait {plan}. Il reste actif jusqu\u{2019}au {ends}. Votre compte passe ensuite au forfait Gratuit, et nous ne vous facturons plus.\n\nPour garder le forfait, choisissez Gérer la facturation sur {} d\u{2019}ici là.\n\n{}",
                context.account_page(),
                context.footer(lang)
            ),
        };
        let subject = match lang {
            Lang::En => format!("Your Noura plan ends on {ends}"),
            Lang::Fr => format!("Votre forfait Noura prend fin le {ends}"),
        };
        (subject, text)
    })
}

/// Why the server ended a paid period early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// A cancellation that takes effect today, with the unused part
    /// refunded.
    Canceled,
    /// The 14-day refund of the latest payment.
    Refunded,
    /// A withdrawal from the contract, in the EU or the UK.
    Withdrawn,
}

/// A paid period that ended today, as its confirmation tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ending<'a> {
    /// Why it ended.
    pub reason: EndReason,
    /// The refund before tax, in the currency's smallest unit.
    pub refund: u64,
    /// The refund's currency.
    pub currency: &'a str,
    /// Today, in Unix seconds.
    pub today: u64,
}

/// The confirmation of a paid period that ended today.
pub fn ended_today(
    context: &Context<'_>,
    langs: MailLang,
    recipient: &str,
    row: &AccountPlanRow,
    ending: Ending<'_>,
) -> BillingMail {
    let Ending {
        reason,
        refund,
        currency,
        today,
    } = ending;
    langs.compose(recipient, |lang| {
        let plan = plan_name(&row.plan, lang);
        let amount = money(refund, currency, lang);
        let on = date(today, lang);
        let (subject, opening) = match (reason, lang) {
            (EndReason::Canceled, Lang::En) => (
                "Your Noura plan is cancelled".to_owned(),
                format!("You cancelled your {plan} plan on {on}, and it ended that day."),
            ),
            (EndReason::Canceled, Lang::Fr) => (
                "Votre forfait Noura est résilié".to_owned(),
                format!("Vous avez résilié votre forfait {plan} le {on}, et il a pris fin ce jour-là."),
            ),
            (EndReason::Refunded, Lang::En) => (
                "Your Noura refund".to_owned(),
                format!("You asked for a refund of your last payment for the {plan} plan on {on}. The period it paid for has ended."),
            ),
            (EndReason::Refunded, Lang::Fr) => (
                "Votre remboursement Noura".to_owned(),
                format!("Vous avez demandé le remboursement de votre dernier paiement pour le forfait {plan} le {on}. La période qu\u{2019}il couvrait a pris fin."),
            ),
            (EndReason::Withdrawn, Lang::En) => (
                "We received your withdrawal".to_owned(),
                format!("We received your withdrawal from your Noura Sync contract, {plan} plan, on {on}."),
            ),
            (EndReason::Withdrawn, Lang::Fr) => (
                "Nous avons reçu votre rétractation".to_owned(),
                format!("Nous avons reçu votre rétractation du contrat Noura Sync, forfait {plan}, le {on}."),
            ),
        };
        let refund_line = match (refund, lang) {
            (0, Lang::En) => "Nothing of the period was left to refund.".to_owned(),
            (0, Lang::Fr) => "Il ne restait rien de la période à rembourser.".to_owned(),
            (_, Lang::En) => format!(
                "We're refunding {amount} plus the tax you paid on it, within 14 days, to the payment method you used. Your bank may take a few more days to show it."
            ),
            (_, Lang::Fr) => format!(
                "Nous vous remboursons {amount}, plus les taxes payées sur ce montant, dans les 14 jours, sur le moyen de paiement utilisé. Votre institution financière peut prendre quelques jours de plus pour l\u{2019}afficher."
            ),
        };
        let free = lang.pick(
            "Your account is now on the Free plan.",
            "Votre compte est maintenant au forfait Gratuit.",
        );
        let text = format!(
            "{opening} {free}\n\n{refund_line}\n\n{}",
            context.footer(lang)
        );
        (subject, text)
    })
}

/// A reminder before a renewal on `renews`, in Unix seconds, of `amount`
/// before tax.
pub fn renewal_reminder(
    context: &Context<'_>,
    langs: MailLang,
    recipient: &str,
    row: &AccountPlanRow,
    renews: u64,
) -> BillingMail {
    let currency = row.currency.clone().unwrap_or_else(|| "usd".into());
    let amount = row.amount.unwrap_or(0);
    let yearly = row.yearly();
    langs.compose(recipient, |lang| {
        let plan = plan_name(&row.plan, lang);
        let on = date(renews, lang);
        let price = money(amount, &currency, lang);
        let (subject, opening) = match (yearly, lang) {
            (true, Lang::En) => (
                format!("Your Noura plan renews on {on}"),
                format!("Your {plan} plan renews on {on}, for {price} plus tax."),
            ),
            (true, Lang::Fr) => (
                format!("Votre forfait Noura se renouvelle le {on}"),
                format!("Votre forfait {plan} se renouvelle le {on}, au prix de {price} plus taxes."),
            ),
            (false, Lang::En) => (
                "Your Noura plan renews every month".to_owned(),
                format!("Your {plan} plan renews every month, for {price} plus tax. The next renewal is on {on}."),
            ),
            (false, Lang::Fr) => (
                "Votre forfait Noura se renouvelle chaque mois".to_owned(),
                format!("Votre forfait {plan} se renouvelle chaque mois, au prix de {price} plus taxes. Le prochain renouvellement a lieu le {on}."),
            ),
        };
        let text = format!(
            "{opening}\n\n{}\n\n{}",
            context.how_to_cancel(lang),
            context.footer(lang)
        );
        (subject, text)
    })
}

/// A notice of a price change on the account's plan, the first or the
/// reminder.
pub fn price_notice(
    context: &Context<'_>,
    langs: MailLang,
    recipient: &str,
    change: &sync_store::PriceChangeRow,
    reminder: bool,
) -> BillingMail {
    langs.compose(recipient, |lang| {
        let plan = plan_name(&change.plan, lang);
        let on = super::terms::date(change.effective_day.saturating_mul(sync_store::plans::DAY), lang);
        let (old, new) = match lang {
            Lang::En => (&change.old_en, &change.new_en),
            Lang::Fr => (&change.old_fr, &change.new_fr),
        };
        let (subject, text) = match lang {
            Lang::En => (
                if reminder {
                    format!("Reminder: the price of your Noura plan changes on {on}")
                } else {
                    format!("The price of your Noura plan changes on {on}")
                },
                format!(
                    "On {on}, this clause of your {plan} plan changes.\n\nNow: {old}\nFrom {on}: {new}\n\nThe new price applies from your first renewal after {on}. You can refuse the change by cancelling your plan, at no cost, until 30 days after {on}: sign in at {} and choose Cancel plan, and we refund the part of the period you paid for and didn't use.\n\n{}",
                    context.account_page(),
                    context.footer(lang)
                ),
            ),
            Lang::Fr => (
                if reminder {
                    format!("Rappel : le prix de votre forfait Noura change le {on}")
                } else {
                    format!("Le prix de votre forfait Noura change le {on}")
                },
                format!(
                    "Le {on}, cette clause de votre forfait {plan} change.\n\nActuellement : {old}\nÀ partir du {on} : {new}\n\nLe nouveau prix s\u{2019}applique à partir de votre premier renouvellement après le {on}. Vous pouvez refuser la modification en résiliant votre forfait, sans frais, jusqu\u{2019}à 30 jours après le {on} : connectez-vous à {} et choisissez Résilier le forfait, et nous vous remboursons la partie de la période payée que vous n\u{2019}avez pas utilisée.\n\n{}",
                    context.account_page(),
                    context.footer(lang)
                ),
            ),
        };
        (subject, text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_and_dates_read_in_each_language() {
        assert_eq!(money(9_600, "usd", Lang::En), "USD 96.00");
        assert_eq!(money(805, "eur", Lang::Fr), "8,05 EUR");
        assert_eq!(date(1_795_766_400, Lang::En), "November 27, 2026");
        assert_eq!(date(1_795_766_400, Lang::Fr), "27 novembre 2026");
        assert_eq!(date(1_798_761_600, Lang::Fr), "1er janvier 2027");
    }

    #[test]
    fn quebec_reads_french_first_then_english() {
        let langs = MailLang::for_account(Some("en"), true);
        let mail = langs.compose("a@example.com", |lang| {
            (
                lang.pick("Hello", "Bonjour").to_owned(),
                lang.pick("English\n", "Français\n").to_owned(),
            )
        });
        assert_eq!(mail.subject, "Bonjour / Hello");
        assert!(mail.body.starts_with("Français\n"));
        assert!(mail.body.ends_with("English\n"));
        let alone = MailLang::for_account(Some("fr"), false);
        assert_eq!((alone.first, alone.both), (Lang::Fr, false));
        assert_eq!(MailLang::for_account(None, false).first, Lang::En);
    }
}
