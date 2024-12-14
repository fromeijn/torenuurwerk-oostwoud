import RPi.GPIO as GPIO
import time
import logging
from logging.handlers import RotatingFileHandler
from os import system, getuid
import datetime
import paho.mqtt.client as mqtt
import json
import numpy as np
import pandas as pd

with open("/home/pi/torenuurwerk-oostwoud/config.json") as f:
    config = json.load(f)

start_timestamp = datetime.datetime.now().timestamp()
slag_timestamp = pd.Timestamp.now()
slag_aantal = 0
slingervanger_state = "inactive"
last_hart_sensor = False

mqtt_topic = "torenuurwerk-oostwoud"

def on_connect(client, userdata, flags, rc):
    logger.info("Connected with result code "+str(rc))
    client.subscribe(mqtt_topic+"/#")

def on_message(client, userdata, msg):
    global slingervanger_state
    if msg.topic == mqtt_topic+"/slingervanger/set":
        logger.info("slingervanger set state: "+str(msg.payload))
        if msg.payload == b"active":
            slingervanger_state = "active"
        elif msg.payload == b"inactive":
            slingervanger_state = "inactive"
        else:
            return
        client.publish(mqtt_topic+"/slingervanger/state", slingervanger(slingervanger_state))

def on_publish(client, userdata, mid):
    pass

def on_subscribe(client, userdata, mid, granted_qos):
    logger.info("Subscribed: "+str(mid)+" "+str(granted_qos))
          
GPIO.setmode(GPIO.BCM)
GPIO.setwarnings(False)

AANTAL_SECONDEN_IN_EEN_DAG = 24*60*60
AANTAL_SECONDEN_IN_EEN_HALFUUR = 0.5*60*60
SLAG_TELLEN_TIMEOUT_SECONDEN = 120
STUUR_UPTIME_INTERVAL_SECONDEN = 60
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
    SLAGWERK_OPHALEN: True,
    GAANTWERK_OPHALEN: True,
    NOOD_EINDE: True,
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
    if actief == "active":
        if waarde_ingang(SLINGERVANGER_UIT):
            logger.info("Slingervanger is al uit")
            return "active"
        logger.info("Slinger vanger wordt uitgezet")
        zet_relay(SLINGERVANGER_RICHTING, True)
        zet_relay(SLINGERVANGER_AAN, True)
    else:
        if waarde_ingang(SLINGERVANGER_IN):
            logger.info("Slingervanger is al ingetrokken")
            return "inactive"
        logger.info("Slinger vanger wordt ingetrokken")
        zet_relay(SLINGERVANGER_RICHTING, False)
        zet_relay(SLINGERVANGER_AAN, True)

    # Relay mag niet te lang aan staan, anders brandt de diode door en wordt de 
    if actief == "active":
        for i in range(10):
            if waarde_ingang(SLINGERVANGER_UIT):
                logger.info("Slingervanger is uit")
                time.sleep(2)
                zet_relay(SLINGERVANGER_AAN, False)
                return "active"
            time.sleep(0.05)
        logger.error("Slingervanger is niet uit")
    else:
        for i in range(10):
            if waarde_ingang(SLINGERVANGER_IN):
                logger.info("Slingervanger is ingetrokken")
                time.sleep(2)
                zet_relay(SLINGERVANGER_AAN, False)
                return "inactive"
            time.sleep(0.2)
        logger.error("Slingervanger is niet ingetrokken")
    
    zet_relay(SLINGERVANGER_AAN, False)
    return "error"

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
    global slag_timestamp
    global slag_aantal
    global last_hart_sensor
    time.sleep(0.2)
    ingang_waarde = bool(waarde_ingang(channel))
    logger.info(f'Wijzinging gedetecteerd voor {inputs_to_names[channel]} ({channel}) naar {ingang_waarde}')
    if channel == NOOD_EINDE and ingang_waarde:
        fout_gedetecteerd = 'Nood einde'
    if channel == GAANTWERK_OPHALEN and ingang_waarde:
        opwinden_vraag_gaand = True
    if channel == SLAGWERK_OPHALEN and ingang_waarde:
        opwinden_vraag_slag = True
    if channel == KNOPJE and ingang_waarde:
        logger.warn("Knopje is ingedrukt, raspberry wordt afgesloten")
        system("shutdown now -h")
    if channel == HART_SENSOR:
        if ingang_waarde and ingang_waarde != last_hart_sensor:
            if slag_timestamp + pd.Timedelta(seconds=SLAG_TELLEN_TIMEOUT_SECONDEN) < pd.Timestamp.now() and slag_aantal == 0:
                slag_timestamp = pd.Timestamp.now()
                slag_aantal = 1
                logger.info(f"Slag wordt geteld {slag_aantal}")
            else:
                slag_aantal = slag_aantal + 1
                logger.info(f"Slag wordt geteld {slag_aantal}")
        last_hart_sensor = ingang_waarde

    if channel == SLINGERVANGER_IN and ingang_waarde:
        client.publish(mqtt_topic+"/slingervanger/state", "inactive")
    if channel == SLINGERVANGER_UIT and ingang_waarde:
        client.publish(mqtt_topic+"/slingervanger/state", "active")

        


if __name__ == "__main__":
    logger = create_rotating_log("/home/pi/torenuurwerk-oostwoud/torenuurwerk-oostwoud.log")
    logger.info("Programma wordt gestart")

    client = mqtt.Client(client_id="torenuurwerk-oostwoud",
                            transport="tcp",
                            protocol=mqtt.MQTTv311,
                            clean_session=True)

    client.on_message = on_message
    client.on_connect = on_connect
    client.on_publish = on_publish
    client.on_subscribe = on_subscribe

    client.username_pw_set(config["mqtt_user"], config["mqtt_password"])
    client.connect(config["mqtt_host"], config["mqtt_port"])
    client.loop_start();


    for i in inputs:
        GPIO.setup(i, GPIO.IN, pull_up_down=GPIO.PUD_UP)
        GPIO.add_event_detect(i, GPIO.BOTH, callback=gpio_changed, bouncetime=200)

    for relay in relays:
        GPIO.setup(relay, GPIO.OUT, initial=True)

    if waarde_ingang(NOOD_EINDE):
        fout_gedetecteerd = 'Nood einde'
        logger.info("Nood einde is ingedrukt, programma wordt gestopt")

    client.publish(mqtt_topic+"/slingervanger/state", slingervanger(slingervanger_state))
    client.publish(mqtt_topic+"/state", "idle")
    client.publish(mqtt_topic+"/uptime", int(datetime.datetime.now().timestamp()-start_timestamp))
    uptime_send_timestamp = datetime.datetime.now().timestamp()

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

        if slag_aantal > 0 and slag_timestamp + pd.Timedelta(seconds=SLAG_TELLEN_TIMEOUT_SECONDEN) < pd.Timestamp.now():
            tijd_verschil_seconden = (slag_timestamp - slag_timestamp.round('30min')).seconds
            if tijd_verschil_seconden > AANTAL_SECONDEN_IN_EEN_HALFUUR:
                tijd_verschil_seconden = tijd_verschil_seconden - AANTAL_SECONDEN_IN_EEN_DAG
            logger.info(f"Slag timestamp: {slag_timestamp} ({slag_timestamp.round('30min').to_pydatetime()}) = {tijd_verschil_seconden}")
            logger.info(f"Slag is geteld {slag_aantal} ({tijd_verschil_seconden})")
            client.publish(mqtt_topic+"/slag", slag_aantal)
            client.publish(mqtt_topic+"/verschil", tijd_verschil_seconden)
            slag_aantal = 0


        if uptime_send_timestamp + STUUR_UPTIME_INTERVAL_SECONDEN < datetime.datetime.now().timestamp():
            uptime_send_timestamp = datetime.datetime.now().timestamp()
            client.publish(mqtt_topic+"/uptime", int(datetime.datetime.now().timestamp()-start_timestamp))
        zet_relay(LED, led_state)
        led_state = not led_state


    client.publish(mqtt_topic+"/state", "error (fout gedetecteerd: "+fout_gedetecteerd+")")
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
