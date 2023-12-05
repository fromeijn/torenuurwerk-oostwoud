import RPi.GPIO as GPIO
import time
import logging
from logging.handlers import RotatingFileHandler
from os import system, getuid


GPIO.setmode(GPIO.BCM)
GPIO.setwarnings(False)

MAXIMALE_OPWINDTIJD_SECONDEN = 60
OPWIND_MOTOR_AAN = 24
OPWIND_MOTOR_SLAG = 22
OPWIND_MOTOR_GAAND = 23
SLINGERVANGER_RICHTING = 27
SLINGERVANGER_AAN = 17
LED = 12

relays =  [
    OPWIND_MOTOR_AAN,
    OPWIND_MOTOR_SLAG,
    OPWIND_MOTOR_GAAND,
    SLINGERVANGER_RICHTING,
    SLINGERVANGER_AAN,
    LED,
]

relays_to_names = {
    OPWIND_MOTOR_AAN: 'OPWIND_MOTOR_AAN',
    OPWIND_MOTOR_SLAG: 'OPWIND_MOTOR_SLAG',
    OPWIND_MOTOR_GAAND: 'OPWIND_MOTOR_GAAND',
    SLINGERVANGER_RICHTING: 'SLINGERVANGER_RICHTING',
    SLINGERVANGER_AAN: 'SLINGERVANGER_AAN',
    LED: 'LED',
}

SLINGERVANGER_IN = 21
SLINGERVANGER_UIT  = 26
SLAGWERK_OPHALEN  = 25
GAANTWERK_OPHALEN  = 20
NOOD_EINDE  = 19
HART_SENSOR = 16
KNOPJE = 5

inputs = [
    SLINGERVANGER_IN,
    SLINGERVANGER_UIT,
    SLAGWERK_OPHALEN,
    GAANTWERK_OPHALEN,
    NOOD_EINDE,
    HART_SENSOR,
    KNOPJE,
]

inputs_to_names = {
    SLINGERVANGER_IN: 'SLINGERVANGER_IN',
    SLINGERVANGER_UIT: 'SLINGERVANGER_UIT',
    SLAGWERK_OPHALEN: 'SLAGWERK_OPHALEN',
    GAANTWERK_OPHALEN: 'GAANTWERK_OPHALEN',
    NOOD_EINDE: 'NOOD_EINDE',
    HART_SENSOR: 'HART_SENSOR',
    KNOPJE: 'KNOPJE',
}

INPUTS_INVERTED = {
    SLINGERVANGER_IN: True,
    SLINGERVANGER_UIT: True,
    SLAGWERK_OPHALEN: False,
    GAANTWERK_OPHALEN: False,
    NOOD_EINDE: False,
    HART_SENSOR: False,
    KNOPJE: True,
}

opwinden_vraag_slag = False
opwinden_vraag_gaand = False
fout_gedetecteerd = None

def create_rotating_log(path):
    """
    Creates a rotating log
    """

    logFormatter = logging.Formatter("%(asctime)s [%(threadName)-12.12s] [%(levelname)-5.5s]  %(message)s")
    logger = logging.getLogger("Rotating Log")
    logger.setLevel(logging.INFO)
    
    # add a rotating handler
    fileHandler = RotatingFileHandler(path, maxBytes=10_000_000,
                                  backupCount=5)
    fileHandler.setFormatter(logFormatter)
    logger.addHandler(fileHandler)

    consoleHandler = logging.StreamHandler()
    consoleHandler.setFormatter(logFormatter)
    logger.addHandler(consoleHandler)
    return logger

def zet_relay(relay, state):
    if(relay != LED):
        logger.info(f'{relays_to_names[relay]} wordt {state} ({(not state)})')
    GPIO.output(relay, (not state))

def zet_meerdere_relays(relays, states):
    items = zip(relays, states)  
    for item in items:
        zet_relay(item[0], item[1])

def waarde_ingang(ingang):
    return GPIO.input(ingang) ^ INPUTS_INVERTED[ingang]

def slingervanger(actief):
    if actief:
        if waarde_ingang(SLINGERVANGER_UIT):
            logger.info("Slinger vanger is al uit")
            return
        logger.info("Slinger vanger wordt uitgezet")
        zet_relay(SLINGERVANGER_RICHTING, True)
        zet_relay(SLINGERVANGER_AAN, True)
    else:
        if waarde_ingang(SLINGERVANGER_IN):
            logger.info("Slinger vanger is al ingetrokken")
            return
        logger.info("Slinger vanger wordt ingetrokken")
        zet_relay(SLINGERVANGER_RICHTING, False)
        zet_relay(SLINGERVANGER_AAN, True)

    # Relay mag niet te lang aan staan, anders brandt de diode door en wordt de 
    if actief:
        for i in range(10):
            if waarde_ingang(SLINGERVANGER_UIT):
                logger.info("Slinger vanger is uit")
                time.sleep(0.1)
                zet_relay(SLINGERVANGER_AAN, False)
                return
            time.sleep(0.05)
        logger.info("Slinger vanger is niet uit")
    else:
        for i in range(10):
            if waarde_ingang(SLINGERVANGER_IN):
                logger.info("Slinger vanger is ingetrokken")
                time.sleep(0.1)
                zet_relay(SLINGERVANGER_AAN, False)
                return
            time.sleep(0.05)
        logger.info("Slinger vanger is niet ingetrokken")
    
    zet_relay(SLINGERVANGER_AAN, False)

def opwinden(werk):
    if werk == 'slag':
        ingang = SLAGWERK_OPHALEN
        uitgang = [True, False]
    elif werk == 'gaand':
        ingang = GAANTWERK_OPHALEN
        uitgang = [False, True]
    else:
        logger.info(f'Onbekend werk {werk}')
        return
    
    if not waarde_ingang(ingang):
        logger.info(f'{werk}werk is al opgewonden')
        return
    logger.info(f'{werk}werk wordt opgewonden')
    zet_meerdere_relays([OPWIND_MOTOR_SLAG, OPWIND_MOTOR_GAAND], uitgang)
    time.sleep(0.2) # Geef de relays de tijd om te schakelen
    zet_relay(OPWIND_MOTOR_AAN, True)

    start_opwinden = time.time()
    while start_opwinden + MAXIMALE_OPWINDTIJD_SECONDEN > time.time() and fout_gedetecteerd is None:
        if not waarde_ingang(ingang):
            zet_relay(OPWIND_MOTOR_AAN, False)
            zet_meerdere_relays([OPWIND_MOTOR_SLAG, OPWIND_MOTOR_GAAND], [False, False])
            logger.info(f"{werk}werk is opgewonden")
            return
        time.sleep(0.1)
    zet_relay(OPWIND_MOTOR_AAN, False)
    zet_meerdere_relays([OPWIND_MOTOR_SLAG, OPWIND_MOTOR_GAAND], [False, False])
    logger.info(f"{werk}werk is niet opgewonden")

def gpio_changed(channel):
    global opwinden_vraag_gaand
    global opwinden_vraag_slag
    global fout_gedetecteerd
    time.sleep(0.2)
    logger.info(f'Wijzinging gedetecteerd voor {inputs_to_names[channel]} ({channel}) naar {waarde_ingang(channel)}')
    if(channel == NOOD_EINDE and waarde_ingang(channel)):
        fout_gedetecteerd = 'Nood einde'
    if(channel == GAANTWERK_OPHALEN and waarde_ingang(channel)):
        opwinden_vraag_gaand = True
    if(channel == SLAGWERK_OPHALEN and waarde_ingang(channel)):
        opwinden_vraag_slag = True
    if(channel == KNOPJE and waarde_ingang(channel)):
        logger.warn("Knopje is ingedrukt, raspberry wordt afgesloten")
        system("shutdown now -h")

if __name__ == "__main__":
    logger = create_rotating_log("/home/pi/torenuurwerk-oostwoud/torenuurwerk-oostwoud.log")
    logger.info("Programma wordt gestart")

    for i in inputs:
        GPIO.setup(i, GPIO.IN, pull_up_down=GPIO.PUD_UP)
        GPIO.add_event_detect(i, GPIO.BOTH, callback=gpio_changed, bouncetime=200)

    for relay in relays:
        GPIO.setup(relay, GPIO.OUT, initial=True)

    if waarde_ingang(NOOD_EINDE):
        fout_gedetecteerd = 'Nood einde'
        logger.info("Nood einde is ingedrukt, programma wordt gestopt")

    slinger_vanger_state = False
    slingervanger(slinger_vanger_state)

    # while True:
    #     input(f"enter next state: {not slinger_vanger_state}")
              
    #     slinger_vanger_state = not slinger_vanger_state
    #     slingervanger(slinger_vanger_state)

    led_state = True
    """ hooft programma loop, werkt zolang er geen fout """
    while fout_gedetecteerd is None:
        if opwinden_vraag_gaand:
            opwinden_vraag_gaand = False
            opwinden('gaand')
        if opwinden_vraag_slag:
            opwinden_vraag_slag = False
            opwinden('slag')
        time.sleep(0.5)
        zet_relay(LED, led_state)
        led_state = not led_state



    logger.info(f"fout gedetecteerd: {fout_gedetecteerd}")
    zet_meerdere_relays(relays, [False, False, False, False, False])

    """ wachten totdat de fout is opgelost """
    i = 0
    while True:
        if(fout_gedetecteerd == 'Nood einde' and not waarde_ingang(NOOD_EINDE)):
            logger.info("Nood einde fout is opgelost, programma wordt herstart")
            exit(0)
        i = i + 1
        if(i>60):
            i = 0
            logger.info(f"fout gedetecteerd: {fout_gedetecteerd}")
        time.sleep(1)

