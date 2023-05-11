#![no_std]
#![feature(type_alias_impl_trait, const_async_blocks)]
#![warn(
    clippy::complexity,
    clippy::correctness,
    clippy::perf,
    clippy::style,
    clippy::undocumented_unsafe_blocks,
    rust_2018_idioms
)]

use asr::{
    future::{next_tick, retry},
    timer,
    timer::TimerState,
    watcher::Watcher,
    Address, FromEndian, Process,
};

asr::panic_handler!();
asr::async_main!(nightly);

async fn main() {
    let settings = Settings::register();

    loop {
        // Hook to the target process
        let process = retry(|| PROCESS_NAMES.into_iter().find_map(Process::attach)).await;

        process.until_closes(async {
            // Once the target has been found and attached to, set up default watchers
            let mut watchers = Watchers::default();

            let wram_base = retry(|| process
                .memory_ranges()
                .find(|x| x.size().unwrap_or_default() == 0x521000)?
                .address().ok()
            ).await + 0x400020;

            loop {
                // Splitting logic. Adapted from OG LiveSplit:
                // Order of execution
                // 1. update() will always be run first. There are no conditions on the execution of this action.
                // 2. If the timer is currently either running or paused, then the isLoading, gameTime, and reset actions will be run.
                // 3. If reset does not return true, then the split action will be run.
                // 4. If the timer is currently not running (and not paused), then the start action will be run.
                update_loop(&mut watchers, &process, wram_base);

                let timer_state = timer::state();
                if timer_state == TimerState::Running || timer_state == TimerState::Paused {
                    if reset(&watchers, &settings) {
                        timer::reset()
                    } else if split(&watchers, &settings) {
                        timer::split()
                    }
                }

                if timer::state() == TimerState::NotRunning && start(&watchers, &settings) {
                    timer::start();
                }

                next_tick().await;
            }
        }).await;
    }
}

#[derive(Default)]
struct Watchers {
    levelid: Watcher<Levels>,
    state: Watcher<u8>,
    end_of_level_flag: Watcher<bool>,
    game_ending_flag: Watcher<bool>,
    time_bonus: Watcher<u16>,
    save_select: Watcher<u8>,
    zone_select: Watcher<u8>,
    save_slot: Watcher<u8>,
}

#[derive(asr::Settings)]
struct Settings {
    #[default = true]
    /// START: Auto start (No save)
    start_nosave: bool,
    #[default = true]
    /// START: Auto start (Clean save)
    start_clean_save: bool,
    #[default = true]
    /// START: Auto start (Angel Island Zone - No clean save)
    start_no_clean_save: bool,
    #[default = true]
    /// START: Auto start (New Game+)
    start_new_game_plus: bool,
    #[default = true]
    /// RESET: Auto reset
    reset: bool,
    #[default = true]
    /// Angel Island Zone - Act 1
    angel_island_1: bool,
    #[default = true]
    /// Angel Island Zone - Act 2
    angel_island_2: bool,
    #[default = true]
    /// Hydrocity Zone - Act 1
    hydrocity_1: bool,
    #[default = true]
    /// Hydrocity Zone - Act 2
    hydrocity_2: bool,
    #[default = true]
    /// Marble Garden Zone - Act 1
    marble_garden_1: bool,
    #[default = true]
    /// Marble Garden Zone - Act 2
    marble_garden_2: bool,
    #[default = true]
    /// Carnival Night Zone - Act 1
    carnival_night_1: bool,
    #[default = true]
    /// Carnival Night Zone - Act 2
    carnival_night_2: bool,
    #[default = true]
    /// Ice Cap Zone - Act 1
    ice_cap_1: bool,
    #[default = true]
    /// Ice Cap Zone - Act 2
    ice_cap_2: bool,
    #[default = true]
    /// Launch Base Zone - Act 1
    launch_base_1: bool,
    #[default = true]
    /// Launch Base Zone - Act 2
    launch_base_2: bool,
    #[default = true]
    /// Mushroom Hill Zone - Act 1
    mushroom_hill_1: bool,
    #[default = true]
    /// Mushroom Hill Zone - Act 2
    mushroom_hill_2: bool,
    #[default = true]
    /// Flying Battery Zone - Act 1
    flying_battery_1: bool,
    #[default = true]
    /// Flying Battery Zone - Act 2
    flying_battery_2: bool,
    #[default = true]
    /// Sandopolis Zone - Act 1
    sandopolis_1: bool,
    #[default = true]
    /// Sandopolis Zone - Act 2
    sandopolis_2: bool,
    #[default = true]
    /// Lava Reef Zone - Act 1
    lava_reef_1: bool,
    #[default = true]
    /// Lava Reef Zone - Act 2
    lava_reef_2: bool,
    #[default = true]
    /// Hidden Palace Zone
    hidden_palace: bool,
    #[default = true]
    /// Sky Sanctuary Zone
    sky_sanctuary: bool,
    #[default = true]
    /// Death Egg Zone - Act 1
    death_egg_1: bool,
    #[default = true]
    /// Death Egg Zone - Act 2
    death_egg_2: bool,
    #[default = true]
    /// Doomsday Zone
    doomsday: bool,
}

fn update_loop(watchers: &mut Watchers, process: &Process, wram_base: Address) {
    // Filtered state variables. They essentially exclude State.InGame
    // Used in order to fix a couple of bugs that will otherwise appear with the start trigger
    let mut state = match &watchers.state.pair { Some(x) => x.current, _ => 0 };
    let mut save_slot = match &watchers.save_slot.pair { Some(x) => x.current, _ => 0 };
    let save_select = process.read::<u8>(wram_base + 0xEF4B).ok().unwrap_or_default();
    let cstate = process.read::<u8>(wram_base + 0xF600).ok().unwrap_or_default();

    if cstate != STATE_INGAME {
        state = cstate;

        if save_select > 0 && save_select <= 8 {
            save_slot = process.read::<u8>(wram_base + 0xE6AC + 0xA * (save_select as u64 - 1)).ok().unwrap_or_default();
        }
    }

    let mut zone_select = match &watchers.zone_select.pair { Some(x) => x.current, _ => 0 };

    if save_select > 0 && save_select <= 8 {
        zone_select = process.read::<u8>(wram_base + 0xB15F + 0x4A * (save_select as u64 - 1)).ok().unwrap_or_default();
    }

    // Define current Act
    // As act = 0 can both mean Angel Island Act 1 and main menu, we need to check if the LevelStarted flag is set.
    // If it's not, keep the old value (old.act) in order to allow splitting after returning to the main menu.
    let mut act = match &watchers.levelid.pair { Some(x) => x.current, _ => Levels::AngelIslandAct1 };

    let temp_act = process.read::<u8>(wram_base + 0xEE4F).ok().unwrap_or_default();
    let temp_zone = process.read::<u8>(wram_base + 0xEE4E).ok().unwrap_or_default();

    act = match temp_act + temp_zone * 10 {
        0 => if process.read::<u8>(wram_base + 0xF711).ok().unwrap_or_default() != 0 { Levels::AngelIslandAct1 } else { act },
        1 => Levels::AngelIslandAct2,
        10 => Levels::HydrocityAct1,
        11 => Levels::HydrocityAct2,
        20 => Levels::MarbleGardenAct1,
        21 => Levels::MarbleGardenAct2,
        30 => Levels::CarnivalNightAct1,
        31 => Levels::CarnivalNightAct2,
        50 => Levels::IceCapAct1,
        51 => Levels::IceCapAct2,
        60 => Levels::LaunchBaseAct1,
        61 => Levels::LaunchBaseAct2,
        70 => Levels::MushroomHillAct1,
        71 => Levels::MushroomHillAct2,
        40 => Levels::FlyingBatteryAct1,
        41 => Levels::FlyingBatteryAct2,
        80 => Levels::SandopolisAct1,
        81 => Levels::SandopolisAct2,
        90 => Levels::LavaReefAct1,
        91 | 220 => Levels::LavaReefAct2,
        221 => Levels::HiddenPalace,
        100 | 101 => Levels::SkySanctuary,
        110 => Levels::DeathEggAct1,
        111 | 230 => Levels::DeathEggAct2,
        120 => Levels::DoomsDay,
        131 => Levels::Ending,
        _ => act,
    };

    // Update the watchers
    watchers.levelid.update_infallible(act);
    watchers.state.update_infallible(state);
    watchers.end_of_level_flag.update_infallible(process.read::<u8>(wram_base + 0xFAA8).ok().unwrap_or_default() != 0);
    watchers.game_ending_flag.update_infallible(process.read::<u8>(wram_base + 0xEF72).ok().unwrap_or_default() != 0);
    watchers.time_bonus.update_infallible(process.read::<u16>(wram_base + 0xF7D2).ok().unwrap_or_default().from_be());
    watchers.save_select.update_infallible(save_select);
    watchers.zone_select.update_infallible(zone_select);
    watchers.save_slot.update_infallible(save_slot);
}

fn start(watchers: &Watchers, settings: &Settings) -> bool {
    let Some(state) = &watchers.state.pair else { return false };

    if state.old == STATE_SAVESELECT && state.current == STATE_LOADING {
        let Some(save_select) = &watchers.save_select.pair else { return false };

        if save_select.current == 0 {
            return settings.start_nosave
        } else {
            let Some(zone_select) = &watchers.zone_select.pair else { return false };

            if zone_select.current == 0 {
                let Some(save_slot) = &watchers.save_select.pair else { return false };
                if save_slot.old == SAVESLOTSTATE_INPROGRESS {
                    return settings.start_no_clean_save
                } else if save_slot.old == SAVESLOTSTATE_NEWGAME {
                    return settings.start_clean_save
                } else if settings.start_new_game_plus {
                    return true
                }
            }
        }
    }
    false
}

fn split(watchers: &Watchers, settings: &Settings) -> bool {
    let Some(act) = &watchers.levelid.pair else { return false };
    let Some(game_ending_flag) = &watchers.game_ending_flag.pair else { return false };

    // If current act is AIZ1 (or an invalid stage) there's no need to continue
    if act.current == Levels::AngelIslandAct1 {
        return false;
    }
    // If current act is 21 (Sky Sanctuary) and the ending flag becomes true, trigger Knuckles' ending
    else if settings.sky_sanctuary && act.current == Levels::SkySanctuary && game_ending_flag.current && !game_ending_flag.old
    {
        return true;
    }

    // Special Trigger for Death Egg Zone Act 2 in Act 1: in this case a split needs to be triggered when the Time Bonus drops to zero, in accordance to speedrun.com rulings
    let Some(time_bonus) = &watchers.time_bonus.pair else { return false };
    let Some(end_level_flag) = &watchers.end_of_level_flag.pair else { return false };
    if settings.death_egg_2 && act.old == Levels::DeathEggAct2 && time_bonus.old != 0 && time_bonus.current == 0 && end_level_flag.current
    {
        return true;
    }

    // Normal splitting condition: trigger a split whenever the act changes
    act.old != act.current && match act.old {
            Levels::AngelIslandAct1 => settings.angel_island_1 && end_level_flag.old,
            Levels::AngelIslandAct2 => settings.angel_island_2,
            Levels::HydrocityAct1 => settings.hydrocity_1,
            Levels::HydrocityAct2 => settings.hydrocity_2,
            Levels::MarbleGardenAct1 => settings.marble_garden_1,
            Levels::MarbleGardenAct2 => settings.marble_garden_2,
            Levels::CarnivalNightAct1 => settings.carnival_night_1,
            Levels::CarnivalNightAct2 => settings.carnival_night_2,
            Levels::IceCapAct1 => settings.ice_cap_1,
            Levels::IceCapAct2 => settings.ice_cap_2,
            Levels::LaunchBaseAct1 => settings.launch_base_1,
            Levels::LaunchBaseAct2 => settings.launch_base_2,
            Levels::MushroomHillAct1 => settings.mushroom_hill_1,
            Levels::MushroomHillAct2 => settings.mushroom_hill_2,
            Levels::FlyingBatteryAct1 => settings.flying_battery_1,
            Levels::FlyingBatteryAct2 => settings.flying_battery_2,
            Levels::SandopolisAct1 => settings.sandopolis_1,
            Levels::SandopolisAct2 => settings.sandopolis_2,
            Levels::LavaReefAct1 => settings.lava_reef_1,
            Levels::LavaReefAct2 => settings.lava_reef_2,
            Levels::HiddenPalace => settings.hidden_palace,
            Levels::SkySanctuary => settings.sky_sanctuary,
            Levels::DeathEggAct1 => settings.death_egg_1,
            Levels::DeathEggAct2 => settings.death_egg_2,
            Levels::DoomsDay => settings.doomsday,
            _ => false,
        }
}

fn reset(watchers: &Watchers, settings: &Settings) -> bool {
    let Some(save_select) = &watchers.save_select.pair else { return false };

    if save_select.current == 0 {
        let Some(state) = &watchers.state.pair else { return false };
        if state.old == STATE_SAVESELECT && state.current == STATE_LOADING {
            return settings.reset
        }
    } else if save_select.current > 0 && save_select.current <= 8 && !save_select.changed() {
        let Some(save_slot) = &watchers.save_slot.pair else { return false };
        if save_slot.old != SAVESLOTSTATE_NEWGAME && save_slot.current == SAVESLOTSTATE_NEWGAME {
            return settings.reset
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Levels {
    AngelIslandAct1,
    AngelIslandAct2,
    HydrocityAct1,
    HydrocityAct2,
    MarbleGardenAct1,
    MarbleGardenAct2,
    CarnivalNightAct1,
    CarnivalNightAct2,
    IceCapAct1,
    IceCapAct2,
    LaunchBaseAct1,
    LaunchBaseAct2,
    MushroomHillAct1,
    MushroomHillAct2,
    FlyingBatteryAct1,
    FlyingBatteryAct2,
    SandopolisAct1,
    SandopolisAct2,
    LavaReefAct1,
    LavaReefAct2,
    HiddenPalace,
    SkySanctuary,
    DeathEggAct1,
    DeathEggAct2,
    DoomsDay,
    Ending,
}

// Consts used in the script
const STATE_SAVESELECT: u8 = 0x4C;
const STATE_LOADING: u8 = 0x8C;
const STATE_INGAME: u8 = 0x0C;
//const STATE_SPECIALSTAGE: u8 = 0x34;
//const STATE_EXITINGSPECIALSTAGE: u8 = 0x48;
const SAVESLOTSTATE_NEWGAME: u8 = 0x80;
const SAVESLOTSTATE_INPROGRESS: u8 = 0x00;
//const SAVESLOTSTATE_COMPLETE: u8 = 0x01;
//const SAVESLOTSTATE_COMPLETEWITHEMERALDS: u8 = 0x02;
//const SAVESLOTSTATE_COMPLETEWITHSUPEREMERALDS: u8 = 0x03;

const PROCESS_NAMES: [&str; 1] = ["Sonic3AIR.exe"];
