use std::collections::HashMap;

use anyhow::{bail, Error};
use lazy_static::lazy_static;

use super::time::*;

use nom::{
    error::{context, ParseError, VerboseError},
    bytes::complete::{tag, take_while1},
    combinator::{map_res, all_consuming, opt, recognize},
    sequence::{pair, preceded, tuple},
    character::complete::{alpha1, space0, digit1},
    multi::separated_nonempty_list,
};

type IResult<I, O, E = VerboseError<I>> = Result<(I, O), nom::Err<E>>;

fn parse_error<'a>(i: &'a str, context: &'static str) -> nom::Err<VerboseError<&'a str>> {
    let err = VerboseError { errors: Vec::new() };
    let err =VerboseError::add_context(i, context, err);
    nom::Err::Error(err)
}

lazy_static! {
    pub static ref TIME_SPAN_UNITS: HashMap<&'static str, f64> = {
        let mut map = HashMap::new();

        let second = 1.0;

        map.insert("seconds", second);
        map.insert("second", second);
        map.insert("sec", second);
        map.insert("s", second);

        let msec = second / 1000.0;

        map.insert("msec", msec);
        map.insert("ms", msec);

        let usec = msec / 1000.0;

        map.insert("usec", usec);
        map.insert("us", usec);
        map.insert("µs", usec);

        let nsec = usec / 1000.0;

        map.insert("nsec", nsec);
        map.insert("ns", nsec);

        let minute = second * 60.0;

        map.insert("minutes", minute);
        map.insert("minute", minute);
        map.insert("min", minute);
        map.insert("m", minute);

        let hour = minute * 60.0;

        map.insert("hours", hour);
        map.insert("hour", hour);
        map.insert("hr", hour);
        map.insert("h", hour);

        let day = hour * 24.0 ;

        map.insert("days", day);
        map.insert("day", day);
        map.insert("d", day);

        let week = day * 7.0;

        map.insert("weeks", week);
        map.insert("week", week);
        map.insert("w", week);

        let month = 30.44 * day;

        map.insert("months", month);
        map.insert("month", month);
        map.insert("M", month);

        let year = 365.25 * day;

        map.insert("years", year);
        map.insert("year", year);
        map.insert("y", year);

        map
    };
}
fn parse_u32(i: &str) -> IResult<&str, u32> {
    map_res(recognize(digit1), str::parse)(i)
}

fn parse_u64(i: &str) -> IResult<&str, u64> {
    map_res(recognize(digit1), str::parse)(i)
}

fn parse_weekday(i: &str) -> IResult<&str, WeekDays, VerboseError<&str>> {
    let (i, text) = alpha1(i)?;

    match text.to_ascii_lowercase().as_str() {
        "monday" | "mon" => Ok((i, WeekDays::MONDAY)),
        "tuesday" | "tue" => Ok((i, WeekDays::TUESDAY)),
        "wednesday" | "wed" => Ok((i, WeekDays::WEDNESDAY)),
        "thursday" | "thu" => Ok((i, WeekDays::THURSDAY)),
        "friday" | "fri" => Ok((i, WeekDays::FRIDAY)),
        "saturday" | "sat" => Ok((i, WeekDays::SATURDAY)),
        "sunday" | "sun" => Ok((i, WeekDays::SUNDAY)),
        _ => return Err(parse_error(text, "weekday")),
    }
}

fn parse_weekdays_range(i: &str) -> IResult<&str, WeekDays> {
    let (i, startday) = parse_weekday(i)?;

    let generate_range = |start, end| {
        let mut res = 0;
        let mut pos = start;
        loop {
            res |= pos;
            if pos >= end { break; }
            pos = pos << 1;
        }
        WeekDays::from_bits(res).unwrap()
    };

    if let (i, Some((_, endday))) = opt(pair(tag(".."),parse_weekday))(i)? {
        let start = startday.bits();
        let end = endday.bits();
        if start > end {
            let set1 = generate_range(start, WeekDays::SUNDAY.bits());
            let set2 = generate_range(WeekDays::MONDAY.bits(), end);
            Ok((i, set1 | set2))
        } else {
            Ok((i, generate_range(start, end)))
        }
    } else {
        Ok((i, startday))
    }
}

fn parse_date_time_comp(i: &str) -> IResult<&str, DateTimeValue> {

    let (i, value) = parse_u32(i)?;

    if let (i, Some(end)) = opt(preceded(tag(".."), parse_u32))(i)? {
        return Ok((i, DateTimeValue::Range(value, end)))
    }

    if i.starts_with("/") {
        let i = &i[1..];
        let (i, repeat) = parse_u32(i)?;
        Ok((i, DateTimeValue::Repeated(value, repeat)))
    } else {
        Ok((i, DateTimeValue::Single(value)))
    }
}

fn parse_date_time_comp_list(i: &str) -> IResult<&str, Vec<DateTimeValue>> {

    if i.starts_with("*") {
        return Ok((&i[1..], Vec::new()));
    }

    separated_nonempty_list(tag(","), parse_date_time_comp)(i)
}

fn parse_time_spec(i: &str) -> IResult<&str, (Vec<DateTimeValue>, Vec<DateTimeValue>, Vec<DateTimeValue>)> {

    let (i, (hour, minute, opt_second)) = tuple((
        parse_date_time_comp_list,
        preceded(tag(":"), parse_date_time_comp_list),
        opt(preceded(tag(":"), parse_date_time_comp_list)),
    ))(i)?;

    if let Some(second) = opt_second {
        Ok((i, (hour, minute, second)))
    } else {
        Ok((i, (hour, minute, vec![DateTimeValue::Single(0)])))
    }
}

pub fn parse_calendar_event(i: &str) -> Result<CalendarEvent, Error> {
    match all_consuming(parse_calendar_event_incomplete)(i) {
        Err(err) => bail!("unable to parse calendar event: {}", err),
        Ok((_, ce)) => Ok(ce),
    }
}

fn parse_calendar_event_incomplete(mut i: &str) -> IResult<&str, CalendarEvent> {

    let mut has_dayspec = false;
    let mut has_timespec = false;
    let has_datespec = false;

    let mut event = CalendarEvent::default();

    if i.starts_with(|c: char| char::is_ascii_alphabetic(&c)) {

        match i {
            "minutely" => {
                return Ok(("", CalendarEvent {
                    second: vec![DateTimeValue::Single(0)],
                    ..Default::default()
                }));
            }
            "hourly" => {
                return Ok(("", CalendarEvent {
                    minute: vec![DateTimeValue::Single(0)],
                    second: vec![DateTimeValue::Single(0)],
                    ..Default::default()
                }));
            }
            "daily" => {
                return Ok(("", CalendarEvent {
                    hour: vec![DateTimeValue::Single(0)],
                    minute: vec![DateTimeValue::Single(0)],
                    second: vec![DateTimeValue::Single(0)],
                    ..Default::default()
                }));
            }
            "monthly" | "weekly" | "yearly" | "quarterly" | "semiannually" => {
                unimplemented!();
            }
            _ => { /* continue */ }
        }

        let (n, range_list) =  context(
            "weekday range list",
            separated_nonempty_list(tag(","), parse_weekdays_range)
        )(i)?;

        has_dayspec = true;

        i = space0(n)?.0;

        for range in range_list  { event.days.insert(range); }
    }

    // todo: support date specs

    if let (n, Some((hour, minute, second))) = opt(parse_time_spec)(i)? {
        event.hour = hour;
        event.minute = minute;
        event.second = second;
        has_timespec = true;
        i = n;
    } else {
        event.hour = vec![DateTimeValue::Single(0)];
        event.minute = vec![DateTimeValue::Single(0)];
        event.second = vec![DateTimeValue::Single(0)];
    }

    if !(has_dayspec || has_timespec || has_datespec) {
        return Err(parse_error(i, "date or time specification"));
    }

    Ok((i, event))
}

fn parse_time_unit(i: &str) ->  IResult<&str, &str> {
    let (n, text) = take_while1(|c: char| char::is_ascii_alphabetic(&c) || c == 'µ')(i)?;
    if TIME_SPAN_UNITS.contains_key(&text) {
        Ok((n, text))
    } else {
        Err(parse_error(text, "time unit"))
    }
}


pub fn parse_time_span(i: &str) -> Result<TimeSpan, Error> {
    match all_consuming(parse_time_span_incomplete)(i) {
        Err(err) => bail!("unable to parse time span: {}", err),
        Ok((_, ts)) => Ok(ts),
    }
}

fn parse_time_span_incomplete(mut i: &str) -> IResult<&str, TimeSpan> {

    let mut ts = TimeSpan::default();

    loop {
        i = space0(i)?.0;
        if i.is_empty() { break; }
        let (n, num) = parse_u64(i)?;
        i = space0(n)?.0;

        if let (n, Some(unit)) = opt(parse_time_unit)(i)? {
            i = n;
            match unit {
                "seconds" | "second" | "sec" | "s" => {
                    ts.seconds += num;
                }
                "msec" | "ms" => {
                    ts.msec += num;
                }
                "usec" | "us" | "µs" => {
                    ts.usec += num;
                }
                "nsec" | "ns" => {
                    ts.nsec += num;
                }
                "minutes" | "minute" | "min" | "m" => {
                    ts.minutes += num;
                }
                "hours" | "hour" | "hr" | "h" => {
                    ts.hours += num;
                }
                "days" | "day" | "d" => {
                    ts.days += num;
                }
                "weeks" | "week" | "w" => {
                    ts.weeks += num;
                }
                "months" | "month" | "M" => {
                    ts.months += num;
                }
                "years" | "year" | "y" => {
                    ts.years += num;
                }
                _ => return Err(parse_error(unit, "internal error")),
            }
        } else {
            ts.seconds += num;
        }
    }

    Ok((i, ts))
}
